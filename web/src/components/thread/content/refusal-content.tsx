import { Ban } from "lucide-react";

interface RefusalContentProps {
  message: string;
}

export function RefusalContent({ message }: RefusalContentProps) {
  return (
    <div className="rounded-md border border-warning/40 bg-warning/10 px-3 py-2">
      <div className="flex items-center gap-2">
        <Ban className="h-4 w-4 text-warning" />
        <span className="text-sm font-medium text-warning-foreground">Refusal</span>
      </div>
      <p className="mt-1 text-sm text-warning-foreground">{message}</p>
    </div>
  );
}
