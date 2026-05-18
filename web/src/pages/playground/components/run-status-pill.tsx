import { Check, CircleAlert, Loader2 } from "lucide-react";
import { cn } from "@/lib/utils";

type Variant = "running" | "finished" | "errored" | "streaming" | "done";

interface Props {
  variant: Variant;
  label?: string;
  className?: string;
}

export function RunStatusPill({ variant, label, className }: Props) {
  const styles: Record<Variant, string> = {
    running: "bg-primary/10 text-primary border-primary/30",
    streaming: "bg-primary/10 text-primary border-primary/30",
    finished: "bg-muted text-muted-foreground border-border",
    done: "bg-muted text-muted-foreground border-border",
    errored: "bg-destructive/10 text-destructive border-destructive/30",
  };
  const text =
    label ??
    (variant === "running" || variant === "streaming"
      ? variant === "running"
        ? "Running"
        : "Streaming"
      : variant === "errored"
        ? "Errored"
        : variant === "done"
          ? "Done"
          : "Finished");
  const Icon =
    variant === "running" || variant === "streaming"
      ? Loader2
      : variant === "errored"
        ? CircleAlert
        : Check;
  const spinning = variant === "running" || variant === "streaming";

  return (
    <span
      className={cn(
        "inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-xs",
        styles[variant],
        className,
      )}
    >
      <Icon className={cn("size-3", spinning && "animate-spin")} />
      {text}
    </span>
  );
}
