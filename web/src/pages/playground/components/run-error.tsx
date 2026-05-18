import { CircleAlert } from "lucide-react";

interface Props {
  code: string;
  message: string;
}

const FRIENDLY: Record<string, string> = {
  agent_busy: "Agent is busy with another run.",
  agent_not_registered: "Agent is no longer registered.",
  invoke_timeout: "The SDK didn't respond in time.",
  cancelled: "Cancelled by user.",
  client_aborted: "Cancelled by user.",
  bad_run_input: "Bad run input.",
  agui_extra_missing: "SDK is missing the [agui] extra.",
  unsupported_runtime: "This agent runtime can't be invoked yet.",
  too_large: "Request body is too large (>4 MiB).",
  network_error: "Network error.",
  internal: "Server error.",
};

export function RunError({ code, message }: Props) {
  const friendly = FRIENDLY[code];
  const display = friendly ?? message;
  return (
    <div className="flex items-start gap-3 rounded-md border border-destructive/30 bg-destructive/10 p-3 text-destructive">
      <CircleAlert className="size-4 shrink-0 mt-0.5" />
      <div className="flex-1 min-w-0">
        <div className="text-sm">{display}</div>
        {friendly && message && message !== friendly && (
          <div className="mt-1 text-xs opacity-80 break-words">{message}</div>
        )}
      </div>
      <code className="font-mono rounded bg-background/40 px-1.5 py-0.5 text-xs">{code}</code>
    </div>
  );
}
