import { Brain } from "lucide-react";

import { omittedReasoningLabel } from "./reasoning-labels";
import { TextContent } from "./text-content";

interface ThinkingContentProps {
  text: string;
  signed?: boolean;
  markdownEnabled?: boolean;
}

export function ThinkingContent({ text, signed, markdownEnabled = true }: ThinkingContentProps) {
  // A reasoning step whose text was withheld is still a step of the conversation: it is shown as an entry of
  // its own, saying whether it was signed, rather than dropped or left as a blank bubble.
  const omitted = omittedReasoningLabel(text, signed);
  if (omitted) {
    return (
      <div className="rounded-md border border-border/50 bg-muted/30 px-3 py-2">
        <div className="flex items-center gap-2">
          <Brain className="h-4 w-4 text-role-thinking" />
          <span className="text-sm text-muted-foreground italic">{omitted}</span>
        </div>
      </div>
    );
  }
  return <TextContent text={text} markdownEnabled={markdownEnabled} />;
}
