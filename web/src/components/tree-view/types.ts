import type { ReactNode } from "react";

export interface TreeNodeData {
  id: string;
  children?: TreeNodeData[];
}

export interface TreeViewProps<T extends TreeNodeData> {
  data: T | null;
  selectedId?: string | null;
  onSelect?: (id: string) => void;
  collapsedIds?: Set<string>;
  onToggleCollapse?: (id: string) => void;
  renderContent: (node: T, state: TreeNodeState) => ReactNode;
  getRowClassName?: (node: T) => string | undefined;
  className?: string;
}

export interface TreeNodeProps<T extends TreeNodeData> {
  node: T;
  depth: number;
  isLast: boolean;
  selectedId?: string | null;
  onSelect?: (id: string) => void;
  collapsedIds?: Set<string>;
  onToggleCollapse?: (id: string) => void;
  renderContent: (node: T, state: TreeNodeState) => ReactNode;
  getRowClassName?: (node: T) => string | undefined;
}

export interface TreeNodeState {
  isSelected: boolean;
  isCollapsed: boolean;
  hasChildren: boolean;
  depth: number;
  isLast: boolean;
}
