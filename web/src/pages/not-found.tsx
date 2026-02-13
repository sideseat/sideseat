import { Link, useNavigate } from "react-router";
import { ArrowLeft, Home, GitBranch, Radio, Zap } from "lucide-react";

import { Button } from "@/components/ui/button";
import { brand } from "@/lib/navigation";

export default function NotFoundPage() {
  const navigate = useNavigate();

  const quickLinks = [
    {
      title: "Home",
      icon: Home,
      url: "/",
    },
    {
      title: "Traces",
      icon: GitBranch,
      url: "/projects/default/observability/traces",
    },
    {
      title: "Realtime",
      icon: Radio,
      url: "/projects/default/observability/realtime",
    },
  ];

  return (
    <div className="flex min-h-screen w-full items-center justify-center bg-background p-4 sm:p-6">
      <div className="relative w-full max-w-lg overflow-hidden rounded-2xl border border-border/60 bg-gradient-to-br from-muted/50 via-background to-muted/30 p-6 shadow-xl sm:rounded-3xl sm:p-10">
        {/* Decorative blurs */}
        <div className="absolute -right-8 -top-8 h-32 w-32 rounded-full bg-primary/10 blur-3xl sm:h-40 sm:w-40" />
        <div className="absolute -bottom-6 -left-6 h-24 w-24 rounded-full bg-primary/5 blur-2xl sm:h-32 sm:w-32" />

        <div className="relative flex flex-col items-center gap-6 text-center sm:gap-8">
          {/* Logo */}
          <div className="flex size-16 items-center justify-center rounded-2xl border border-border/60 bg-card shadow-sm sm:size-20">
            <img
              src="/ui/icons/android-chrome-192x192.png"
              alt={brand.name}
              className="size-10 rounded-lg sm:size-12"
            />
          </div>

          {/* Content */}
          <div className="space-y-2 sm:space-y-3">
            <div className="inline-flex items-center gap-1.5 rounded-full border border-border/60 bg-muted/50 px-3 py-1">
              <span className="text-xs font-semibold text-muted-foreground sm:text-sm">404</span>
              <span className="size-1 rounded-full bg-muted-foreground/40" />
              <span className="text-xs font-medium text-muted-foreground sm:text-sm">
                Not Found
              </span>
            </div>
            <h1 className="text-xl font-semibold tracking-tight sm:text-2xl">Page not found</h1>
            <p className="mx-auto max-w-xs text-sm text-muted-foreground sm:text-base">
              The page you're looking for doesn't exist or has been moved.
            </p>
          </div>

          {/* Primary actions */}
          <div className="flex w-full flex-col gap-2 sm:flex-row sm:justify-center">
            <Button
              onClick={() => navigate(-1)}
              variant="outline"
              className="w-full gap-2 sm:w-auto"
            >
              <ArrowLeft className="size-4" />
              Go Back
            </Button>
            <Button asChild className="w-full gap-2 sm:w-auto">
              <Link to="/">
                <Home className="size-4" />
                Home
              </Link>
            </Button>
          </div>

          {/* Divider */}
          <div className="flex w-full items-center gap-3">
            <div className="h-px flex-1 bg-border/60" />
            <span className="text-xs text-muted-foreground">or navigate to</span>
            <div className="h-px flex-1 bg-border/60" />
          </div>

          {/* Quick links */}
          <div className="flex flex-wrap justify-center gap-2">
            {quickLinks.map((link) => {
              const Icon = link.icon;
              return (
                <Button
                  key={link.title}
                  variant="ghost"
                  size="sm"
                  asChild
                  className="gap-2 text-muted-foreground hover:text-foreground"
                >
                  <Link to={link.url}>
                    <Icon className="size-4" />
                    {link.title}
                  </Link>
                </Button>
              );
            })}
          </div>

          {/* Footer */}
          <p className="text-xs text-muted-foreground sm:text-sm">
            Need help?{" "}
            <a
              href={brand.docsUrl}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-1 font-medium underline underline-offset-4 hover:text-foreground"
            >
              <Zap className="size-3" />
              Documentation
            </a>
          </p>
        </div>
      </div>
    </div>
  );
}
