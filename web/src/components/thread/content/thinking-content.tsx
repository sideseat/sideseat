import { Brain } from "lucide-react";

import { TextContent } from "./text-content";

interface ThinkingContentProps {
  text: string;
  signed?: boolean;
  markdownEnabled?: boolean;
}

/** The label for reasoning whose text the model withheld and only signed. */
export const WITHHELD_REASONING_LABEL = "Reasoning: text omitted, signed";

export function ThinkingContent({ text, signed, markdownEnabled = true }: ThinkingContentProps) {
  // Reasoning the model signed without showing its text is still a turn of the conversation: the next request
  // re-sends it, so it is shown as an entry of its own rather than dropped or left as a blank bubble.
  if (text.trim() === "" && signed) {
    return (
      <div className="rounded-md border border-border/50 bg-muted/30 px-3 py-2">
        <div className="flex items-center gap-2">
          <Brain className="h-4 w-4 text-role-thinking" />
          <span className="text-sm text-muted-foreground italic">{WITHHELD_REASONING_LABEL}</span>
        </div>
      </div>
    );
  }
  return <TextContent text={text} markdownEnabled={markdownEnabled} />;
}
