import { createContext } from "react";
import type { TreeNode, ViewMode } from "../lib/types";

export interface TraceViewContextValue {
  tree: TreeNode | null;
  filteredTree: TreeNode | null;
  traceStart: Date | null;
  traceDuration: number;
  rootDuration: number;
  rootCost: number;

  selectedSpanId: string | null;
  setSelectedSpanId: (id: string | null) => void;
  selectedNode: TreeNode | null;

  collapsedNodes: Set<string>;
  toggleCollapsed: (id: string) => void;
  expandAll: () => void;
  collapseAll: () => void;
  allExpanded: boolean;

  viewMode: ViewMode;
  setViewMode: (mode: ViewMode) => void;

  showDuration: boolean;
  showCost: boolean;

  showNonGenAiSpans: boolean;
  setShowNonGenAiSpans: (show: boolean) => void;
}

export const TraceViewContext = createContext<TraceViewContextValue | null>(null);
