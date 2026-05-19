import { Bug, Plus } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useParams } from "react-router";

import { Button } from "@/components/ui/button";
import { useAgentRun } from "@/api/agui/use-agent-run";
import { useRegistrationsList, usePresenceStream } from "@/api/registrations/hooks";
import { cn } from "@/lib/utils";
import { usePageToolbar } from "@/lib/page-toolbar";

import { AgentEmpty } from "./components/agent-empty";
import { Composer } from "./components/composer";
import { DebugPanel } from "./components/debug-panel";
import { LandingView } from "./components/landing-view";
import { MessageList } from "./components/message-list";

export default function PlaygroundPage() {
  const { projectId = "default" } = useParams<{ projectId: string }>();
  const presence = usePresenceStream(projectId);
  const listing = useRegistrationsList(projectId);

  // All invokable kinds render uniformly. Mcp is excluded (not invokable
  // via the AG-UI run-agent path).
  const entries = useMemo(
    () =>
      [
        ...(listing.data?.agents ?? []),
        ...(listing.data?.graphs ?? []),
        ...(listing.data?.swarms ?? []),
      ].sort((a, b) => a.name.localeCompare(b.name)),
    [listing.data],
  );

  const [selectedName, setSelectedName] = useState<string | null>(null);
  const [debugOpen, setDebugOpen] = useState(false);
  const [focusKey, setFocusKey] = useState(0);

  const run = useAgentRun({ projectId, agentName: selectedName });

  useEffect(() => {
    if (selectedName) setFocusKey((k) => k + 1);
  }, [selectedName]);

  // Drop the selection if the registration disappears (e.g., the SDK
  // disconnected). Don't auto-select — the user picks the card.
  useEffect(() => {
    if (selectedName && !entries.some((a) => a.name === selectedName)) {
      setSelectedName(null);
    }
  }, [entries, selectedName]);

  const inChat = selectedName !== null && (run.state.messages.length > 0 || run.isStreaming);

  const handleNewChat = useCallback(() => {
    run.clear();
    setSelectedName(null);
  }, [run]);

  const loading =
    listing.data === undefined && presence.status !== "error" && presence.status !== "disconnected";

  const showCenter = entries.length > 0 || loading;

  const toolbar = useMemo(
    () => (
      <>
        {inChat && (
          <Button variant="outline" size="sm" className="h-8 gap-1.5" onClick={handleNewChat}>
            <Plus className="h-3.5 w-3.5" />
            <span className="hidden sm:inline">New chat</span>
          </Button>
        )}
        <Button
          variant="outline"
          size="sm"
          className={cn("h-8 gap-1.5", debugOpen && "bg-accent text-accent-foreground")}
          onClick={() => setDebugOpen((v) => !v)}
          aria-pressed={debugOpen}
        >
          <Bug className="h-3.5 w-3.5" />
          <span className="hidden sm:inline">Debug</span>
        </Button>
      </>
    ),
    [inChat, handleNewChat, debugOpen],
  );
  usePageToolbar(showCenter ? toolbar : null);

  return (
    <div className="flex flex-1 flex-col pt-header-offset sm:pt-header-offset-sm">
      {!showCenter ? (
        <div className="flex flex-1 items-center justify-center px-4 py-8">
          <AgentEmpty />
        </div>
      ) : inChat ? (
        <>
          <MessageList state={run.state} isStreaming={run.isStreaming} />
          <ComposerBar
            run={run}
            disabled={selectedName === null}
            placeholder={`Message ${selectedName}…`}
            focusKey={focusKey}
          />
        </>
      ) : (
        <>
          {/* Cards: centered horizontally; vertically centered when they
              fit, scrollable otherwise. The composer is rendered outside
              this scroll region so it stays anchored to the bottom. */}
          <div className="flex flex-1 justify-center overflow-y-auto px-4 py-6 sm:py-10">
            <div className="flex w-full max-w-3xl items-center">
              <LandingView
                entries={entries}
                selected={selectedName}
                onSelect={setSelectedName}
                loading={loading}
              />
            </div>
          </div>
          <ComposerBar
            run={run}
            disabled={selectedName === null}
            placeholder={
              selectedName ? `Message ${selectedName}…` : "Pick an agent above to start chatting"
            }
            focusKey={focusKey}
            rows={3}
          />
        </>
      )}
      <DebugPanel open={debugOpen} onOpenChange={setDebugOpen} state={run.state} />
    </div>
  );
}

interface ComposerBarProps {
  run: ReturnType<typeof useAgentRun>;
  disabled: boolean;
  placeholder: string;
  focusKey: number;
  rows?: number;
}

function ComposerBar({ run, disabled, placeholder, focusKey, rows }: ComposerBarProps) {
  return (
    <div className="sticky bottom-0 z-20 border-t bg-background/85 backdrop-blur supports-backdrop-filter:bg-background/70">
      <div className="mx-auto w-full max-w-3xl px-3 py-3 md:px-6 md:py-4">
        <Composer
          onSend={run.send}
          onCancel={run.cancel}
          isStreaming={run.isStreaming}
          disabled={disabled}
          placeholder={placeholder}
          focusKey={focusKey}
          rows={rows}
        />
      </div>
    </div>
  );
}
