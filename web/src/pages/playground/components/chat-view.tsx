import type { ChatState } from "@/api/agui/types";
import { MessageList } from "./message-list";

interface Props {
  state: ChatState;
  isStreaming: boolean;
}

export function ChatView({ state, isStreaming }: Props) {
  return <MessageList state={state} isStreaming={isStreaming} />;
}
