import { Link, useNavigate } from "react-router";
import {
  ArrowLeft,
  Home,
  LayoutDashboard,
  Activity,
  FileText,
  Search,
  Sparkles,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { brand } from "@/lib/navigation";

export default function NotFoundPage() {
  const navigate = useNavigate();

  const quickLinks = [
    {
      title: "Dashboard",
      description: "View your operational overview",
      icon: LayoutDashboard,
      url: "/dashboard",
    },
    {
      title: "Traces",
      description: "Monitor AI model traces",
      icon: Activity,
      url: "/traces",
    },
    {
      title: "Prompts",
      description: "Manage prompt library",
      icon: FileText,
      url: "/prompts",
    },
  ];

  return (
    <div className="mx-auto flex min-h-[calc(100vh-var(--header-offset))] w-full max-w-6xl flex-col items-center justify-center gap-8 px-4 py-4 sm:min-h-[calc(100vh-var(--header-offset-sm))] sm:gap-10 sm:px-6 sm:py-6">
      {/* Hero Section */}
      <div className="flex flex-col items-center gap-5 text-center">
        <div className="relative">
          <div className="absolute inset-0 animate-pulse rounded-full bg-primary/20 blur-3xl" />
          <div className="relative flex size-28 items-center justify-center rounded-full border-2 border-dashed border-muted-foreground/30 bg-muted/50 sm:size-32">
            <div className="flex size-16 items-center justify-center rounded-full bg-background sm:size-20">
              <Search className="size-8 text-muted-foreground sm:size-10" />
            </div>
          </div>
        </div>

        <div className="space-y-3 sm:space-y-4">
          <div className="inline-flex items-center gap-2 rounded-full border border-border bg-muted/40 px-4 py-1.5">
            <span className="text-sm font-medium text-muted-foreground">404</span>
            <span className="size-1 rounded-full bg-muted-foreground/40" />
            <span className="text-sm font-medium text-muted-foreground">Page Not Found</span>
          </div>

          <h1 className="text-balance text-3xl font-bold tracking-tight sm:text-5xl">
            Lost in the AI wilderness?
          </h1>

          <p className="mx-auto max-w-2xl text-pretty text-base text-muted-foreground sm:text-lg">
            The page you're looking for doesn't exist or has been moved. Let's get you back on
            track.
          </p>
        </div>

        {/* Primary Actions */}
        <div className="flex flex-wrap items-center justify-center gap-2 sm:gap-3">
          <Button onClick={() => navigate(-1)} variant="outline" className="gap-2 sm:h-11 sm:px-8">
            <ArrowLeft className="size-4" />
            Go Back
          </Button>
          <Button asChild className="gap-2 sm:h-11 sm:px-8">
            <Link to="/">
              <Home className="size-4" />
              Back to Home
            </Link>
          </Button>
        </div>
      </div>

      {/* Quick Links Section */}
      <div className="w-full space-y-4">
        <div className="flex items-center justify-center gap-2 text-center">
          <Sparkles className="size-4 text-muted-foreground" />
          <h2 className="text-sm font-medium text-muted-foreground">Quick Navigation</h2>
        </div>

        <div className="grid gap-3 sm:grid-cols-3 sm:gap-4">
          {quickLinks.map((link) => {
            const Icon = link.icon;
            return (
              <Link key={link.title} to={link.url}>
                <Card className="group relative overflow-hidden transition-all hover:border-primary/50 hover:shadow-md">
                  <div className="absolute inset-0 bg-gradient-to-br from-primary/5 to-transparent opacity-0 transition-opacity group-hover:opacity-100" />
                  <CardHeader className="relative space-y-2.5 p-4 sm:space-y-3 sm:p-6">
                    <div className="flex size-9 items-center justify-center rounded-lg bg-primary/10 text-primary transition-colors group-hover:bg-primary/20 sm:size-10">
                      <Icon className="size-4 sm:size-5" />
                    </div>
                    <div className="space-y-1">
                      <CardTitle className="text-sm sm:text-base">{link.title}</CardTitle>
                      <CardDescription className="text-xs sm:text-sm">
                        {link.description}
                      </CardDescription>
                    </div>
                  </CardHeader>
                </Card>
              </Link>
            );
          })}
        </div>
      </div>

      {/* Footer Info */}
      <div className="text-center text-sm text-muted-foreground">
        <p>
          Need help?{" "}
          <a
            href={brand.docsUrl}
            target="_blank"
            rel="noopener noreferrer"
            className="font-medium underline underline-offset-4 hover:text-foreground"
          >
            Visit our documentation
          </a>
        </p>
      </div>
    </div>
  );
}
