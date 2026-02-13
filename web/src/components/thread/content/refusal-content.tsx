import { Ban } from "lucide-react";

interface RefusalContentProps {
  message: string;
}

export function RefusalContent({ message }: RefusalContentProps) {
  return (
    <div className="rounded-md border border-amber-300 bg-amber-50 dark:border-amber-800 dark:bg-amber-900/30 px-3 py-2">
      <div className="flex items-center gap-2">
        <Ban className="h-4 w-4 text-amber-600 dark:text-amber-400" />
        <span className="text-sm font-medium text-amber-700 dark:text-amber-300">Refusal</span>
      </div>
      <p className="mt-1 text-sm text-amber-700 dark:text-amber-300">{message}</p>
    </div>
  );
}
