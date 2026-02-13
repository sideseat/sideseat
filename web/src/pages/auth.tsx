import { useState, useEffect, useCallback } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { useAuth } from "@/auth/context";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Field, FieldLabel, FieldDescription, FieldError } from "@/components/ui/field";

const authSchema = z.object({
  token: z
    .string()
    .transform((val) => val.trim())
    .pipe(z.string().min(1, "Token is required")),
});

type AuthFormInput = z.input<typeof authSchema>;
type AuthFormOutput = z.output<typeof authSchema>;

export default function AuthPage() {
  const { login, authenticated, loading } = useAuth();
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();

  const redirectUri = searchParams.get("redirect_uri") || "/";
  const tokenFromUrl = searchParams.get("token");

  const [isUrlTokenLoading, setIsUrlTokenLoading] = useState(!!tokenFromUrl);

  const {
    register,
    handleSubmit,
    setError,
    reset,
    formState: { errors, isValid, isSubmitting },
  } = useForm<AuthFormInput, unknown, AuthFormOutput>({
    resolver: zodResolver(authSchema),
    mode: "onChange",
    defaultValues: { token: "" },
  });

  // If already authenticated, redirect
  useEffect(() => {
    if (!loading && authenticated) {
      navigate(decodeURIComponent(redirectUri), { replace: true });
    }
  }, [authenticated, loading, navigate, redirectUri]);

  const handleLogin = useCallback(
    async (tokenToUse: string) => {
      const success = await login(tokenToUse);

      if (success) {
        navigate(decodeURIComponent(redirectUri), { replace: true });
      } else {
        reset({ token: "" });
        setError("token", {
          type: "server",
          message: "Invalid token. Please check the token and try again.",
        });
      }
    },
    [login, navigate, redirectUri, setError, reset],
  );

  // Auto-exchange token from URL (always try, even if already authenticated)
  useEffect(() => {
    if (tokenFromUrl && !loading) {
      setIsUrlTokenLoading(true);
      handleLogin(tokenFromUrl).finally(() => setIsUrlTokenLoading(false));
    }
  }, [tokenFromUrl, loading, handleLogin]);

  const onSubmit = async (data: AuthFormOutput) => {
    await handleLogin(data.token);
  };

  // Show loading while checking initial auth state
  if (loading) {
    return (
      <div className="flex items-center justify-center min-h-screen">
        <div className="text-muted-foreground">Loading...</div>
      </div>
    );
  }

  const isLoading = isSubmitting || isUrlTokenLoading;

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
          {tokenFromUrl && isUrlTokenLoading ? (
            <div className="text-center py-4 text-muted-foreground">Authenticating...</div>
          ) : (
            <form onSubmit={handleSubmit(onSubmit)} className="space-y-4">
              <Field data-invalid={!!errors.token}>
                <FieldLabel htmlFor="token">Token</FieldLabel>
                <Input
                  id="token"
                  type="text"
                  placeholder="Paste your token here"
                  {...register("token")}
                  disabled={isLoading}
                  autoComplete="off"
                  aria-invalid={!!errors.token}
                  autoFocus
                />
                <FieldDescription>
                  Copy the token from the terminal URL when you started the server.
                </FieldDescription>
                {errors.token && <FieldError>{errors.token.message}</FieldError>}
              </Field>

              <Button type="submit" className="w-full" disabled={isLoading || !isValid}>
                {isLoading ? "Authenticating..." : "Authenticate"}
              </Button>
            </form>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
