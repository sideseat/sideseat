import ReactMarkdown from "react-markdown";

interface TextContentProps {
  text: string;
  markdownEnabled?: boolean;
}

export function TextContent({ text, markdownEnabled = true }: TextContentProps) {
  if (!markdownEnabled) {
    return <div className="whitespace-pre-wrap break-words text-sm">{text}</div>;
  }

  return (
    <div className="prose max-w-none overflow-hidden break-words">
      <ReactMarkdown
        components={{
          code: ({ children }) => (
            <code className="rounded bg-muted px-1 py-0.5 font-mono text-xs">{children}</code>
          ),
          pre: ({ children }) => (
            <pre className="my-3 overflow-auto rounded-md bg-muted/50 p-3">{children}</pre>
          ),
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
}
