import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

interface Props {
  status: "disconnected" | "connecting" | "connected" | "error";
}

export function ConnectionIndicator({ status }: Props) {
  const color =
    status === "connected"
      ? "bg-green-500"
      : status === "connecting"
        ? "bg-amber-500"
        : status === "error"
          ? "bg-amber-500"
          : "bg-muted-foreground";

  const label =
    status === "connected"
      ? "Live"
      : status === "connecting"
        ? "Connecting"
        : status === "error"
          ? "Reconnecting"
          : "Offline";

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span
          className={cn("inline-block size-2 rounded-full", color)}
          aria-label={`Presence ${label}`}
        />
      </TooltipTrigger>
      <TooltipContent>Presence: {label}</TooltipContent>
    </Tooltip>
  );
}
