import { cn } from "@/lib/utils";

interface Props {
  className?: string;
}

/** Soft pulse used to mark live/streaming state. Mirrors engagement-mck. */
export function PulseDot({ className }: Props) {
  return (
    <span className={cn("relative inline-flex size-1.5 shrink-0", className)} aria-hidden="true">
      <span className="absolute inset-0 rounded-full bg-primary opacity-75 motion-safe:animate-ping" />
      <span className="relative inline-flex size-full rounded-full bg-primary" />
    </span>
  );
}
