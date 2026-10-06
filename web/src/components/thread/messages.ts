import type { Block } from "@/api/otel/types";
import { getBlockCopyText, getBlockPreview, structuredData } from "./thread-utils";
import { rowConfig, type RowConfig } from "./row-config";

/**
 * One message of the conversation, with the content blocks it is made of.
 *
 * The server returns one entry per content block, because a block is what a rule reads. A reader sees a
 * conversation of messages: an assistant turn that answers in text and then calls two tools is one
 * message, not three. Grouping happens here so the thread shows what the model sent.
 */
export interface ThreadMessage {
  /** Stable across renders and unique within a thread; the first block's key. */
  key: string;
  /** In the order the server returned them. */
  blocks: Block[];
  /** The block the row takes its role, model and time from. */
  lead: Block;
  /** Index of the message's last block in the flat list, for anything placed between messages. */
  lastBlockIndex: number;
}

/** The message a block belongs to: its span, and the message's index within that span. */
function messageKey(block: Block): string {
  return `${block.trace_id}\u0000${block.span_id}\u0000${block.message_index}`;
}

/**
 * The blocks grouped into messages, in order.
 *
 * Only adjacent blocks group. The server may return the same message index twice around something else -
 * a replayed history turn, a span error - and merging across that gap would reorder the conversation.
 */
export function groupBlocksIntoMessages(blocks: Block[]): ThreadMessage[] {
  const messages: ThreadMessage[] = [];
  let currentKey: string | null = null;
  blocks.forEach((block, index) => {
    const key = messageKey(block);
    const last = messages[messages.length - 1];
    if (last && key === currentKey) {
      last.blocks.push(block);
      last.lastBlockIndex = index;
      return;
    }
    currentKey = key;
    messages.push({
      key: `${key}\u0000${block.entry_index}`,
      blocks: [block],
      lead: block,
      lastBlockIndex: index,
    });
  });
  return messages;
}

/** Whether any block of the message reports an error, which colours the whole row. */
export function messageHasError(message: ThreadMessage): boolean {
  return message.blocks.some((block) => block.is_error);
}

/** The first finish reason any of the message's blocks reports. */
export function messageFinishReason(message: ThreadMessage): string | undefined {
  return message.blocks.find((block) => block.finish_reason)?.finish_reason;
}

/** The first model any of the message's blocks names; one message is one model call. */
export function messageModel(message: ThreadMessage): string | undefined {
  return message.blocks.find((block) => block.model)?.model;
}

/**
 * What a collapsed message shows.
 *
 * One block keeps its own preview. Several blocks lead with the text the message says, because that is
 * what a reader scans for, and count the rest by kind so a collapsed row still says a tool was called.
 */
export function getMessagePreview(message: ThreadMessage): string {
  if (message.blocks.length === 1) return getBlockPreview(message.blocks[0]);
  const parts: string[] = [];
  const counts = new Map<string, number>();
  for (const block of message.blocks) {
    if (block.entry_type === "text" || structuredData(block) !== undefined) {
      if (parts.length === 0) parts.push(getBlockPreview(block));
      continue;
    }
    const kind = PART_NAMES[block.entry_type] ?? block.entry_type.replace(/_/g, " ");
    counts.set(kind, (counts.get(kind) ?? 0) + 1);
  }
  for (const [kind, count] of counts) {
    parts.push(count === 1 ? `1 ${kind}` : `${count} ${kind}s`);
  }
  return parts.join(" · ");
}

const PART_NAMES: Record<string, string> = {
  tool_use: "tool call",
  tool_result: "tool result",
  thinking: "thinking block",
  redacted_thinking: "thinking block",
  tool_definitions: "tool definition list",
  image: "image",
  audio: "audio clip",
  video: "video",
  document: "document",
  file: "file",
};

/** The whole message as text, each block as it would be copied on its own. */
export function getMessageCopyText(message: ThreadMessage): string {
  return message.blocks.map(getBlockCopyText).join("\n\n");
}

/**
 * How a message's row presents itself.
 *
 * A message of one block keeps that block's own presentation, so a lone tool call still reads as a tool
 * call. A message of several blocks is named by its role: it is one turn that did several things, and
 * labelling it after its first block would call an answer-and-two-calls turn a "Tool Call".
 */
export function messageRowConfig(message: ThreadMessage): RowConfig {
  if (message.blocks.length === 1) {
    return rowConfig(message.lead.entry_type, message.lead.role);
  }
  return { ...rowConfig("", message.lead.role), showMetadata: true };
}
