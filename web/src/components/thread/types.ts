import type { Block, MessagesMetadata } from "@/api/otel/types";
import type { TokenBreakdown, CostBreakdown } from "@/components/breakdown-popover";

export type ThreadTab = "messages" | "tools";

export interface ThreadViewProps {
  blocks: Block[];
  metadata?: Partial<MessagesMetadata>;
  toolDefinitions?: Record<string, unknown>[];
  tokenBreakdown?: TokenBreakdown;
  costBreakdown?: CostBreakdown;
  isLoading?: boolean;
  error?: Error | null;
  onRetry?: () => void;
  className?: string;
  activeTab?: ThreadTab;
  onTabChange?: (tab: ThreadTab) => void;
  /** Project ID for resolving file references in media content */
  projectId?: string;
  /** Show trace number links on messages (for session view) */
  showTraceLinks?: boolean;
}

export interface ThreadHeaderProps {
  metadata?: Partial<MessagesMetadata>;
  tokenBreakdown?: TokenBreakdown;
  costBreakdown?: CostBreakdown;
  activeTab: ThreadTab;
  onTabChange: (tab: ThreadTab) => void;
  allExpanded: boolean;
  onToggleExpandAll: () => void;
  markdownEnabled: boolean;
  onMarkdownToggle: () => void;
}
