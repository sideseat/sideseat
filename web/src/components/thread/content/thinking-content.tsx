import { TextContent } from "./text-content";

interface ThinkingContentProps {
  text: string;
  markdownEnabled?: boolean;
}

export function ThinkingContent({ text, markdownEnabled = true }: ThinkingContentProps) {
  return <TextContent text={text} markdownEnabled={markdownEnabled} />;
}
