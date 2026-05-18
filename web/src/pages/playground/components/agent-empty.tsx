interface Props {
  title?: string;
  hint?: string;
  command?: string;
}

const DEFAULT_COMMAND =
  "uv run --directory misc/samples/python/strands strands strands_ws --sideseat";

export function AgentEmpty({
  title = "Waiting for agents",
  hint = "Run a sample with the --sideseat flag to see agents here:",
  command = DEFAULT_COMMAND,
}: Props) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center text-center py-10">
      <div className="rounded-lg border bg-card px-6 py-6 max-w-xl">
        <h2 className="text-base font-semibold">{title}</h2>
        <p className="mt-1 text-xs text-muted-foreground">{hint}</p>
        <pre className="mt-3 overflow-x-auto rounded bg-muted px-3 py-2 text-xs text-left">
          {command}
        </pre>
      </div>
    </div>
  );
}
