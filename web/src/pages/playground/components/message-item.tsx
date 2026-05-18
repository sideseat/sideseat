import { Separator } from "@/components/ui/separator";
import { cn } from "@/lib/utils";
import type { Message } from "@/api/agui/types";
import { ReasoningBlock } from "./reasoning-block";
import { RunError } from "./run-error";
import { RunStatusPill } from "./run-status-pill";
import { StateCard } from "./state-card";
import { ToolCallCard } from "./tool-call-card";

interface Props {
  message: Message;
}

export function MessageItem({ message }: Props) {
  switch (message.kind) {
    case "text":
      return <TextBubble message={message} />;
    case "tool_call":
      return <ToolCallCard message={message} />;
    case "reasoning":
      return <ReasoningBlock message={message} />;
    case "step":
      return (
        <div className="flex items-center gap-2 text-xs text-muted-foreground py-1">
          <Separator className="flex-1" />
          <span className="px-1">{message.stepName}</span>
          <Separator className="flex-1" />
        </div>
      );
    case "run_status":
      return (
        <div className="flex justify-center py-1">
          <RunStatusPill variant={message.phase === "started" ? "running" : "finished"} />
        </div>
      );
    case "error":
      return <RunError code={message.code} message={message.message} />;
    case "state":
      return <StateCard payload={message.payload} />;
  }
}

function TextBubble({
  message,
}: {
  message: Extract<Message, { kind: "text" }>;
}) {
  const isUser = message.role === "user";
  return (
    <div className={cn("flex w-full", isUser && "justify-end")}>
      <div
        className={cn(
          "max-w-2xl whitespace-pre-wrap rounded-lg px-3 py-2 text-sm",
          isUser
            ? "bg-primary text-primary-foreground"
            : "bg-muted text-foreground",
        )}
      >
        {message.content || (message.streaming ? "…" : "")}
      </div>
    </div>
  );
}
