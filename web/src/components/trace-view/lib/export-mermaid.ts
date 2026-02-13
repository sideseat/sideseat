import type { Node, Edge } from "@xyflow/react";
import type { SpanNodeData } from "../components/span-node";
import { sanitizeId } from "./export-utils";

/**
 * Generate Mermaid flowchart syntax from diagram nodes and edges.
 * IDs are sanitized but not prefixed (backward compatible).
 *
 * @param nodes - ReactFlow nodes with SpanNodeData
 * @param edges - ReactFlow edges connecting nodes
 * @returns Mermaid flowchart syntax string
 */
export function generateMermaid(nodes: Node<SpanNodeData>[], edges: Edge[]): string {
  if (nodes.length === 0) return "flowchart LR";

  const lines: string[] = ["flowchart LR"];
  const idMap = new Map<string, string>();

  for (const node of nodes) {
    const safeId = sanitizeId(node.id); // No prefix for Mermaid (backward compatible)
    idMap.set(node.id, safeId);
    const label = node.data?.label ?? node.id;
    // Escape Mermaid special characters: quotes, brackets, newlines, and # (styling)
    const safeLabel = label
      .replace(/[\r\n]+/g, " ")
      .replace(/"/g, "'")
      .replace(/[[\]()#]/g, "");
    lines.push(`  ${safeId}["${safeLabel}"]`);
  }

  for (const edge of edges) {
    const sourceId = idMap.get(edge.source) ?? sanitizeId(edge.source);
    const targetId = idMap.get(edge.target) ?? sanitizeId(edge.target);
    lines.push(`  ${sourceId} --> ${targetId}`);
  }

  return lines.join("\n");
}
