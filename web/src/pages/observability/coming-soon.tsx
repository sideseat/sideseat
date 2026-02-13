import { Construction } from "lucide-react";

interface ComingSoonPageProps {
  title: string;
  description?: string;
}

export function ComingSoonPage({ title, description }: ComingSoonPageProps) {
  return (
    <div className="flex h-[calc(100vh-var(--header-height))] w-full items-center justify-center px-4">
      <div className="flex flex-col items-center gap-6 text-center">
        <div className="rounded-full bg-muted p-4">
          <Construction className="h-8 w-8 text-muted-foreground" />
        </div>
        <div className="space-y-2">
          <h1 className="text-2xl font-semibold">{title}</h1>
          <p className="max-w-md text-muted-foreground">
            {description ?? "This feature is coming soon. Check back later for updates."}
          </p>
        </div>
      </div>
    </div>
  );
}

export function RealtimePage() {
  return (
    <ComingSoonPage
      title="Realtime Monitoring"
      description="Watch your AI agents and LLM calls in real-time as they happen."
    />
  );
}
