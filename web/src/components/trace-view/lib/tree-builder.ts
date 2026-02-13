import type { SpanDetail } from "@/api/otel/types";
import type { TreeNode, SpanType } from "./types";

/** ID used for synthetic root node when multiple trace roots exist */
export const VIRTUAL_ROOT_ID = "__virtual_root__";

function getSpanType(span: SpanDetail): SpanType {
  // Check observation_type first (AI-specific classification)
  switch (span.observation_type) {
    case "generation":
      return "llm";
    case "tool":
      return "tool";
    case "agent":
      return "agent";
    case "embedding":
      return "embedding";
    case "retriever":
      return "retriever";
  }

  // Fallback to span_category (includes external services)
  switch (span.span_category) {
    case "llm":
      return "llm";
    case "tool":
      return "tool";
    case "agent":
      return "agent";
    case "http":
      return "http";
    case "db":
      return "db";
  }

  return "span";
}

function calculateDepths(node: TreeNode, depth: number): void {
  node.depth = depth;
  // Virtual root's children should be at depth 0 (root level)
  const childDepth = node.isVirtualRoot ? 0 : depth + 1;
  for (const child of node.children) {
    calculateDepths(child, childDepth);
  }
}

function aggregateMetrics(node: TreeNode): void {
  // First recursively aggregate children (depth-first)
  for (const child of node.children) {
    aggregateMetrics(child);
  }

  // Aggregation strategy based on span type:
  //
  // 1. LLM spans (type="llm"): Use their own metrics. These represent actual
  //    API calls with authoritative token/cost data.
  //
  // 2. Non-LLM spans with children: Aggregate from children. Wrapper spans
  //    (agents, tools, generic spans) should not have their own token/cost
  //    data. If they do (some frameworks duplicate data on parent+child),
  //    we ignore the parent's data to prevent double-counting.
  //
  // 3. Leaf spans without children: Keep their own metrics (may be 0).
  //
  // This approach is universal across frameworks (Strands, LangChain, etc.)
  // because it relies on semantic span classification, not fragile heuristics.

  const isLlmSpan = node.type === "llm";

  if (!isLlmSpan && node.children.length > 0) {
    // Non-LLM parent: aggregate from children
    node.totalTokens = 0;
    node.totalCost = 0;
    for (const child of node.children) {
      node.totalTokens += child.totalTokens;
      node.totalCost += child.totalCost;
    }
  }
  // LLM spans and leaf nodes keep their own metrics
}

function sortChildrenByStartTime(node: TreeNode): void {
  node.children.sort((a, b) => a.startTime.getTime() - b.startTime.getTime());
  for (const child of node.children) {
    sortChildrenByStartTime(child);
  }
}

export function buildTree(spans: SpanDetail[]): TreeNode | null {
  if (spans.length === 0) return null;

  const nodeMap = new Map<string, TreeNode>();

  for (const span of spans) {
    const startTime = new Date(span.timestamp_start);
    const endTime = span.timestamp_end ? new Date(span.timestamp_end) : undefined;
    const duration =
      span.duration_ms ??
      (endTime ? Math.max(0, endTime.getTime() - startTime.getTime()) : undefined);

    nodeMap.set(span.span_id, {
      id: span.span_id,
      name: span.span_name,
      type: getSpanType(span),
      startTime,
      endTime,
      duration,
      children: [],
      depth: 0,
      span,
      totalTokens: span.total_tokens,
      totalCost: span.total_cost,
    });
  }

  // Collect roots (spans without parent) and build parent-child relationships
  const roots: TreeNode[] = [];
  const orphans: TreeNode[] = [];
  // Map trace_id to its root for orphan assignment
  const traceRoots = new Map<string, TreeNode>();

  for (const span of spans) {
    const node = nodeMap.get(span.span_id)!;
    if (span.parent_span_id === null) {
      roots.push(node);
      traceRoots.set(span.trace_id, node);
    } else {
      const parent = nodeMap.get(span.parent_span_id);
      if (parent) {
        parent.children.push(node);
      } else {
        orphans.push(node);
      }
    }
  }

  // Attach orphans to their trace's root, or first root if no match
  for (const orphan of orphans) {
    const traceRoot = traceRoots.get(orphan.span.trace_id);
    if (traceRoot) {
      traceRoot.children.push(orphan);
    } else if (roots.length > 0) {
      roots[0].children.push(orphan);
    } else {
      roots.push(orphan);
    }
  }

  if (roots.length === 0) return null;

  // Sort roots by start time
  roots.sort((a, b) => a.startTime.getTime() - b.startTime.getTime());

  // Single root: return as-is
  if (roots.length === 1) {
    const root = roots[0];
    calculateDepths(root, 0);
    sortChildrenByStartTime(root);
    aggregateMetrics(root);
    return root;
  }

  // Multiple roots: create virtual root to hold them
  const firstRoot = roots[0];
  const lastRoot = roots[roots.length - 1];
  const virtualRoot: TreeNode = {
    id: VIRTUAL_ROOT_ID,
    name: "Session",
    type: "span",
    startTime: firstRoot.startTime,
    endTime: lastRoot.endTime,
    duration: undefined,
    children: roots,
    depth: 0,
    span: firstRoot.span, // Use first root's span for fallback metadata
    totalTokens: 0,
    totalCost: 0,
    isVirtualRoot: true,
  };

  calculateDepths(virtualRoot, 0);
  sortChildrenByStartTime(virtualRoot);
  aggregateMetrics(virtualRoot);

  return virtualRoot;
}

export function flattenTree(node: TreeNode, collapsedNodes: Set<string>): TreeNode[] {
  // Skip virtual root - flatten its children directly
  if (node.isVirtualRoot) {
    const result: TreeNode[] = [];
    for (const child of node.children) {
      result.push(...flattenTree(child, collapsedNodes));
    }
    return result;
  }

  const result: TreeNode[] = [node];

  if (!collapsedNodes.has(node.id)) {
    for (const child of node.children) {
      result.push(...flattenTree(child, collapsedNodes));
    }
  }

  return result;
}

// GenAI span types for filtering
const GENAI_SPAN_TYPES: Set<SpanType> = new Set(["llm", "tool", "agent", "embedding", "retriever"]);

function isGenAiSpan(type: SpanType): boolean {
  return GENAI_SPAN_TYPES.has(type);
}

/**
 * Filter tree to show only GenAI spans and their ancestors.
 * Non-GenAI spans are kept if they have GenAI descendants.
 */
export function filterTree(node: TreeNode, showNonGenAi: boolean): TreeNode | null {
  if (showNonGenAi) return node;

  const filteredChildren = node.children
    .map((child) => filterTree(child, showNonGenAi))
    .filter((child): child is TreeNode => child !== null);

  // Virtual root: always keep if any children remain
  if (node.isVirtualRoot) {
    return filteredChildren.length > 0 ? { ...node, children: filteredChildren } : null;
  }

  // Keep node if GenAI OR has GenAI descendants
  if (isGenAiSpan(node.type) || filteredChildren.length > 0) {
    return { ...node, children: filteredChildren };
  }
  return null;
}
