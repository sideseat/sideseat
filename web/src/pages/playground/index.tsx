import { Plus } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useParams } from "react-router";
import { Button } from "@/components/ui/button";
import { Toggle } from "@/components/ui/toggle";
import { useAgentRun } from "@/api/agui/use-agent-run";
import { useRegistrationsList, usePresenceStream } from "@/api/registrations/hooks";
import { AgentEmpty } from "./components/agent-empty";
import { ChatView } from "./components/chat-view";
import { Composer } from "./components/composer";
import { ConnectionIndicator } from "./components/connection-indicator";
import { DebugPanel } from "./components/debug-panel";
import { LandingView } from "./components/landing-view";

export default function PlaygroundPage() {
  const { projectId = "default" } = useParams<{ projectId: string }>();
  const presence = usePresenceStream(projectId);
  const listing = useRegistrationsList(projectId);
  const agents = useMemo(
    () => [...(listing.data?.agents ?? [])].sort((a, b) => a.name.localeCompare(b.name)),
    [listing.data],
  );

  const [selectedName, setSelectedName] = useState<string | null>(null);
  const [debugOpen, setDebugOpen] = useState(false);
  const [focusKey, setFocusKey] = useState(0);

  const run = useAgentRun({ projectId, agentName: selectedName });

  useEffect(() => {
    if (selectedName) setFocusKey((k) => k + 1);
  }, [selectedName]);

  useEffect(() => {
    if (selectedName && !agents.some((a) => a.name === selectedName)) {
      setSelectedName(null);
    }
  }, [agents, selectedName]);

  const inChat = selectedName !== null && (run.state.messages.length > 0 || run.isStreaming);

  const handleNewChat = useCallback(() => {
    run.clear();
    setSelectedName(null);
  }, [run]);

  const loading =
    listing.data === undefined &&
    presence.status !== "error" &&
    presence.status !== "disconnected";

  const showCenter = agents.length > 0 || loading;

  const toolbar = (
    <div className="flex items-center gap-2">
      <ConnectionIndicator status={presence.status} />
      {inChat && (
        <Button variant="outline" size="sm" className="h-8 gap-1.5" onClick={handleNewChat}>
          <Plus className="h-3.5 w-3.5" />
          <span className="hidden sm:inline">New chat</span>
        </Button>
      )}
      <Toggle
        size="sm"
        pressed={debugOpen}
        onPressedChange={setDebugOpen}
        aria-label="Toggle debug panel"
        className="h-8 px-2 text-xs"
      >
        Debug
      </Toggle>
    </div>
  );

  return (
    <div className="flex flex-1 flex-col min-h-0">
      <div className="flex flex-1 flex-col items-center min-h-0 overflow-hidden">
        {!showCenter ? (
          <div className="flex flex-1 w-full max-w-4xl px-4 md:px-6">
            <AgentEmpty />
          </div>
        ) : inChat ? (
          <div className="flex flex-1 flex-col w-full max-w-4xl min-h-0 px-4 md:px-6 pt-3">
            <div className="flex items-center justify-end pb-2">{toolbar}</div>
            <ChatView state={run.state} isStreaming={run.isStreaming} />
            <Composer
              onSend={run.send}
              onCancel={run.cancel}
              isStreaming={run.isStreaming}
              disabled={selectedName === null}
              placeholder={`Message ${selectedName}…`}
              focusKey={focusKey}
            />
          </div>
        ) : (
          <div className="flex flex-1 flex-col w-full max-w-2xl items-stretch justify-center min-h-0 px-4 md:px-6">
            <div className="flex items-center justify-end pt-3">{toolbar}</div>
            <LandingView
              agents={agents}
              selected={selectedName}
              onSelect={setSelectedName}
              loading={loading}
            />
            <Composer
              onSend={run.send}
              onCancel={run.cancel}
              isStreaming={run.isStreaming}
              disabled={selectedName === null}
              placeholder={
                selectedName
                  ? `Message ${selectedName}…`
                  : "Pick an agent above to start chatting"
              }
              focusKey={focusKey}
            />
          </div>
        )}
      </div>
      <DebugPanel open={debugOpen} onOpenChange={setDebugOpen} state={run.state} />
    </div>
  );
}
