import type { SpanDetail } from "@/api/otel/types";

export type SpanType =
  | "llm"
  | "tool"
  | "agent"
  | "embedding"
  | "retriever"
  | "http"
  | "db"
  | "span";

export interface TreeNode {
  id: string;
  name: string;
  type: SpanType;

  startTime: Date;
  endTime?: Date;
  duration?: number;

  children: TreeNode[];
  depth: number;

  span: SpanDetail;

  totalTokens: number;
  totalCost: number;

  /** True if this is a synthetic root created to hold multiple trace roots */
  isVirtualRoot?: boolean;
}

export type ViewMode = "tree" | "timeline" | "diagram";

export type LayoutDirection = "horizontal" | "vertical";
