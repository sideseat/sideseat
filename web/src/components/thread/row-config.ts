import {
  User,
  Bot,
  Settings,
  Wrench,
  CornerDownRight,
  Brain,
  ListTree,
  AlertCircle,
  HelpCircle,
  Braces,
} from "lucide-react";

/** How a row presents itself: which icon and label it leads with, and whether it names the model. */
export interface RowConfig {
  icon: typeof User;
  label: string;
  accent: string;
  showMetadata: boolean;
}

// Role-based configuration (primary)
const ROLE_CONFIG: Record<string, RowConfig> = {
  system: {
    icon: Settings,
    label: "System",
    accent: "text-role-system",
    showMetadata: true,
  },
  user: {
    icon: User,
    label: "User",
    accent: "text-role-user",
    showMetadata: true,
  },
  assistant: {
    icon: Bot,
    label: "Assistant",
    accent: "text-role-assistant",
    showMetadata: true,
  },
  tool: {
    icon: CornerDownRight,
    label: "Tool Result",
    accent: "text-role-tool",
    showMetadata: false,
  },
};

// Special entry types that override role-based labels
const SPECIAL_ENTRY_CONFIG: Record<string, RowConfig> = {
  tool_use: {
    icon: Wrench,
    label: "Tool Call",
    accent: "text-role-tool-call",
    showMetadata: false,
  },
  tool_result: {
    icon: CornerDownRight,
    label: "Tool Result",
    accent: "text-role-tool",
    showMetadata: false,
  },
  thinking: {
    icon: Brain,
    label: "Thinking",
    accent: "text-role-thinking",
    showMetadata: false,
  },
  redacted_thinking: {
    icon: Brain,
    label: "Thinking",
    accent: "text-role-thinking/50",
    showMetadata: false,
  },
  tool_definitions: {
    icon: ListTree,
    label: "System",
    accent: "text-role-system",
    showMetadata: false,
  },
  refusal: {
    icon: AlertCircle,
    label: "Assistant",
    accent: "text-destructive",
    showMetadata: false,
  },
  json: {
    icon: Braces,
    label: "Structured output",
    accent: "text-role-assistant",
    showMetadata: true,
  },
};

// Default config for unknown types
const DEFAULT_CONFIG: RowConfig = {
  icon: HelpCircle,
  label: "Assistant",
  accent: "text-role-assistant",
  showMetadata: false,
};

/**
 * How a row of this entry type and role presents itself.
 *
 * A special entry type wins over the role: a tool call is a tool call whichever role carried it.
 */
export function rowConfig(entryType: string, role: string): RowConfig {
  return SPECIAL_ENTRY_CONFIG[entryType] ?? ROLE_CONFIG[role] ?? DEFAULT_CONFIG;
}
