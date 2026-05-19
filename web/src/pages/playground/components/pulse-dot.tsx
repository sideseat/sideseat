import { cn } from "@/lib/utils";

interface Props {
  size?: number;
  className?: string;
}

/** Soft pulse used to mark live/streaming state. Mirrors engagement-mck. */
export function PulseDot({ size = 1.5, className }: Props) {
  const px = `${size * 4}px`;
  return (
    <span
      className={cn("relative inline-flex shrink-0", className)}
      style={{ width: px, height: px }}
      aria-hidden="true"
    >
      <span className="absolute inset-0 rounded-full bg-primary opacity-75 motion-safe:animate-ping" />
      <span className="relative inline-flex size-full rounded-full bg-primary" />
    </span>
  );
}
