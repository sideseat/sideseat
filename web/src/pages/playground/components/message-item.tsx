/* Adapted from engagement-mck/solution/site/src/components/chat/message-item.tsx */
import { AlertTriangle } from "lucide-react";
import { memo } from "react";
import { TextContent } from "@/components/thread/content/text-content";
import type { Message } from "@/api/agui/types";
import { ReasoningBlock } from "./reasoning-block";
import { ToolCallCard } from "./tool-call-card";

interface MessageItemProps {
  message: Message;
}

function MessageItemImpl({ message }: MessageItemProps) {
  if (message.kind === "text") {
    const isUser = message.role === "user";
    if (isUser) {
      return (
        <div className="flex w-full justify-end">
          <div className="max-w-2xl rounded-2xl bg-primary px-4 py-2.5 text-sm leading-relaxed text-primary-foreground shadow-xs animate-in fade-in slide-in-from-bottom-1 duration-200">
            <p className="whitespace-pre-wrap break-words">{message.content}</p>
          </div>
        </div>
      );
    }
    // Empty + not streaming → the message produced no visible output
    // (typical when the model only emitted tool calls). Drop the row so
    // we don't render an orphan "Thinking…" placeholder forever.
    if (!message.content && !message.streaming) return null;
    return (
      <div className="w-full text-sm leading-relaxed text-foreground animate-in fade-in slide-in-from-bottom-1 duration-200">
        {message.content ? (
          <TextContent text={message.content} />
        ) : (
          <div className="flex items-center gap-1.5 py-0.5 text-xs text-muted-foreground">
            <span className="size-1.5 animate-pulse rounded-full bg-primary" />
            Thinking…
          </div>
        )}
      </div>
    );
  }
  if (message.kind === "reasoning") {
    return <ReasoningBlock content={message.content} streaming={message.streaming} />;
  }
  if (message.kind === "tool_call") {
    // Standalone tool calls (not inside a group) get the inline <details>.
    return (
      <ToolCallCard
        toolName={message.toolName}
        args={message.args}
        result={message.result}
        done={message.done}
      />
    );
  }
  if (message.kind === "error") {
    return (
      <div className="flex items-start gap-2 rounded-lg border border-destructive/20 bg-destructive/5 px-3.5 py-2.5 text-sm text-destructive shadow-xs">
        <AlertTriangle className="mt-0.5 size-4 shrink-0" />
        <span className="leading-relaxed">{message.message}</span>
      </div>
    );
  }
  // step / run_status / state: rendered elsewhere or intentionally ignored.
  return null;
}

export const MessageItem = memo(MessageItemImpl);
