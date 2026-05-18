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
      return;
    }
    // Auto-select first agent (alphabetical) so the composer is usable
    // immediately and the user doesn't have to click a card just to type.
    if (!selectedName && agents.length > 0) {
      setSelectedName(agents[0].name);
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

  // Page-controlled actions in the layout's main toolbar (extensible slot).
  // Debug is always available; New chat only when there's a chat to clear.
  const toolbar = useMemo(
    () => (
      <>
        {inChat && (
          <Button
            variant="outline"
            size="sm"
            className="h-8 gap-1.5"
            onClick={handleNewChat}
          >
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
  // Show toolbar in chat AND landing modes; only suppress on the AgentEmpty
  // state where there's nothing to debug yet.
  usePageToolbar(showCenter ? toolbar : null);

  return (
    <div className="flex flex-1 flex-col pt-header-offset sm:pt-header-offset-sm">
      {!showCenter ? (
        <div className="flex flex-1 items-center justify-center px-4 py-8">
          <AgentEmpty />
        </div>
      ) : inChat ? (
        // Chat mode: messages flex-1 to push the composer to the viewport
        // bottom on short conversations; sticky keeps it visible once the
        // page scrolls.
        <>
          <MessageList state={run.state} isStreaming={run.isStreaming} />
          <div className="sticky bottom-0 z-20 border-t bg-background/85 backdrop-blur supports-backdrop-filter:bg-background/70">
            <div className="mx-auto w-full max-w-4xl px-3 py-3 md:px-6 md:py-4">
              <Composer
                onSend={run.send}
                onCancel={run.cancel}
                isStreaming={run.isStreaming}
                disabled={selectedName === null}
                placeholder={`Message ${selectedName}…`}
                focusKey={focusKey}
              />
            </div>
          </div>
        </>
      ) : (
        // Landing mode: cards + composer centered as one hero block.
        <div className="flex flex-1 items-center justify-center px-4 py-8">
          <div className="flex w-full max-w-2xl flex-col gap-4">
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
              rows={4}
            />
          </div>
        </div>
      )}
      <DebugPanel open={debugOpen} onOpenChange={setDebugOpen} state={run.state} />
    </div>
  );
}
