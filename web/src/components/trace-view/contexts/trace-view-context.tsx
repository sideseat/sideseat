import { useState, useCallback, useMemo, type ReactNode } from "react";
import type { SpanDetail } from "@/api/otel/types";
import type { TreeNode, ViewMode } from "../lib/types";
import { buildTree, filterTree, VIRTUAL_ROOT_ID } from "../lib/tree-builder";
import { getTraceTimeRange } from "../lib/timeline-calculations";
import { TraceViewContext, type TraceViewContextValue } from "./context";
import { settings, TRACE_VIEW_SHOW_NON_GENAI_KEY } from "@/lib/settings";

interface TraceViewProviderProps {
  spans: SpanDetail[];
  children: ReactNode;
  initialViewMode?: ViewMode;
  onViewModeChange?: (mode: ViewMode) => void;
}

function buildNodeMap(node: TreeNode, map: Map<string, TreeNode>): void {
  map.set(node.id, node);
  for (const child of node.children) {
    buildNodeMap(child, map);
  }
}

function firstSelectableNode(tree: TreeNode | null): TreeNode | null {
  if (!tree) return null;
  return tree.isVirtualRoot && tree.children.length > 0 ? tree.children[0] : tree;
}

export function TraceViewProvider({
  spans,
  children,
  initialViewMode,
  onViewModeChange,
}: TraceViewProviderProps) {
  const tree = useMemo(() => buildTree(spans), [spans]);

  const nodeMap = useMemo(() => {
    const map = new Map<string, TreeNode>();
    if (tree) {
      buildNodeMap(tree, map);
    }
    return map;
  }, [tree]);

  const { traceStart, traceDuration } = useMemo(() => {
    if (!tree) return { traceStart: null, traceDuration: 0 };
    const range = getTraceTimeRange(tree);
    return { traceStart: range.start, traceDuration: range.duration };
  }, [tree]);

  const rootDuration = tree?.duration ?? traceDuration;
  const rootCost = tree?.totalCost ?? 0;

  // GenAI filter state
  const [showNonGenAiSpans, setShowNonGenAiSpansInternal] = useState(
    () => settings.get<boolean>(TRACE_VIEW_SHOW_NON_GENAI_KEY) ?? false,
  );

  const setShowNonGenAiSpans = useCallback((show: boolean) => {
    settings.set(TRACE_VIEW_SHOW_NON_GENAI_KEY, show);
    setShowNonGenAiSpansInternal(show);
  }, []);

  // Compute filtered tree ONCE here (not in each view)
  const filteredTree = useMemo(
    () => (tree ? filterTree(tree, showNonGenAiSpans) : null),
    [tree, showNonGenAiSpans],
  );

  // Build map of visible nodes for O(1) visibility checks
  const filteredNodeMap = useMemo(() => {
    const map = new Map<string, TreeNode>();
    if (filteredTree) {
      buildNodeMap(filteredTree, map);
    }
    return map;
  }, [filteredTree]);

  const [selectedSpanId, setSelectedSpanId] = useState<string | null>(
    () => firstSelectableNode(filteredTree)?.id ?? null,
  );
  const [collapsedNodes, setCollapsedNodes] = useState<Set<string>>(new Set());
  const [allExpanded, setAllExpanded] = useState(true);
  const [internalViewMode, setInternalViewMode] = useState<ViewMode>(initialViewMode ?? "tree");
  const viewMode = initialViewMode ?? internalViewMode;

  const setViewMode = useCallback(
    (mode: ViewMode) => {
      setInternalViewMode(mode);
      onViewModeChange?.(mode);
    },
    [onViewModeChange],
  );

  const selectableTree = showNonGenAiSpans ? tree : filteredTree;
  const selectableNodes = showNonGenAiSpans ? nodeMap : filteredNodeMap;
  const fallbackSpanId = firstSelectableNode(selectableTree)?.id ?? null;
  if (
    (selectedSpanId === null && fallbackSpanId !== null) ||
    (selectedSpanId !== null && !selectableNodes.has(selectedSpanId))
  ) {
    setSelectedSpanId(fallbackSpanId);
  }

  const selectedNode = useMemo(() => {
    if (!selectedSpanId) return null;
    return nodeMap.get(selectedSpanId) ?? null;
  }, [selectedSpanId, nodeMap]);

  const toggleCollapsed = useCallback((id: string) => {
    setCollapsedNodes((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  }, []);

  const expandAll = useCallback(() => {
    setCollapsedNodes(new Set());
    setAllExpanded(true);
  }, []);

  const collapseAll = useCallback(() => {
    // Exclude virtual root from collapsed set (it's never rendered)
    const ids = [...nodeMap.keys()].filter((id) => id !== VIRTUAL_ROOT_ID);
    setCollapsedNodes(new Set(ids));
    setAllExpanded(false);
  }, [nodeMap]);

  const value = useMemo<TraceViewContextValue>(
    () => ({
      tree,
      filteredTree,
      traceStart,
      traceDuration,
      rootDuration,
      rootCost,
      selectedSpanId,
      setSelectedSpanId,
      selectedNode,
      collapsedNodes,
      toggleCollapsed,
      expandAll,
      collapseAll,
      allExpanded,
      viewMode,
      setViewMode,
      showDuration: true,
      showCost: true,
      showNonGenAiSpans,
      setShowNonGenAiSpans,
    }),
    [
      tree,
      filteredTree,
      traceStart,
      traceDuration,
      rootDuration,
      rootCost,
      selectedSpanId,
      selectedNode,
      collapsedNodes,
      toggleCollapsed,
      expandAll,
      collapseAll,
      allExpanded,
      viewMode,
      setViewMode,
      showNonGenAiSpans,
      setShowNonGenAiSpans,
    ],
  );

  return <TraceViewContext.Provider value={value}>{children}</TraceViewContext.Provider>;
}
