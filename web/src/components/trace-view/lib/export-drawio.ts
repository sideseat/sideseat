import type { Node, Edge } from "@xyflow/react";
import type { SpanNodeData } from "../components/span-node";
import type { SpanType } from "./types";
import { SPAN_TYPE_CONFIG } from "./span-config";
import { NODE_WIDTH, NODE_HEIGHT } from "./diagram-utils";
import { sanitizeId, escapeXml, normalizeCoordinates } from "./export-utils";

// Prefix constants to avoid collision with draw.io reserved IDs (0, 1)
const NODE_PREFIX = "n_";
const EDGE_PREFIX = "e_";

function getNodeStyle(type: SpanType): string {
  const config = SPAN_TYPE_CONFIG[type] ?? SPAN_TYPE_CONFIG.span;
  const fillColor = config.hexColor;
  return [
    "rounded=1",
    "whiteSpace=wrap",
    "html=1",
    `fillColor=${fillColor}`,
    `strokeColor=${fillColor}`,
    "fontColor=#FFFFFF",
    "fontSize=12",
    "fontStyle=1", // bold
  ].join(";");
}

/**
 * Generate draw.io (diagrams.net) XML from diagram nodes and edges.
 * Nodes are styled by span type with matching hex colors.
 * IDs are prefixed with n_ (nodes) and e_ (edges) to avoid draw.io reserved IDs.
 *
 * @param nodes - ReactFlow nodes with SpanNodeData
 * @param edges - ReactFlow edges connecting nodes
 * @returns draw.io XML string (mxfile format)
 */
export function generateDrawIO(nodes: Node<SpanNodeData>[], edges: Edge[]): string {
  if (nodes.length === 0) {
    return buildDrawIOXml([]);
  }

  // Normalize coordinates to start from padding offset
  const normalizedNodes = normalizeCoordinates(nodes);

  const cells: string[] = [];
  const idMap = new Map<string, string>();

  // Generate node cells with n_ prefix
  for (const node of normalizedNodes) {
    const safeId = NODE_PREFIX + sanitizeId(node.id);
    idMap.set(node.id, safeId);

    const label = escapeXml(node.data?.label ?? node.id);
    const type = (node.data?.type as SpanType) ?? "span";
    const style = getNodeStyle(type);
    const { x, y } = node.position;

    cells.push(
      `        <mxCell id="${safeId}" value="${label}" style="${style}" vertex="1" parent="1">` +
        `<mxGeometry x="${x}" y="${y}" width="${NODE_WIDTH}" height="${NODE_HEIGHT}" as="geometry"/></mxCell>`,
    );
  }

  // Generate edge cells with e_ prefix
  // Only include edges where both source and target nodes exist (weren't filtered out)
  const edgeStyle =
    "edgeStyle=orthogonalEdgeStyle;rounded=1;orthogonalLoop=1;jettySize=auto;html=1;strokeColor=#888888;strokeWidth=2;";

  for (const edge of edges) {
    const sourceId = idMap.get(edge.source);
    const targetId = idMap.get(edge.target);

    // Skip edges where source or target was filtered out by normalizeCoordinates
    if (!sourceId || !targetId) continue;

    const safeId = EDGE_PREFIX + sanitizeId(edge.id);
    cells.push(
      `        <mxCell id="${safeId}" style="${edgeStyle}" edge="1" parent="1" source="${sourceId}" target="${targetId}">` +
        `<mxGeometry relative="1" as="geometry"/></mxCell>`,
    );
  }

  return buildDrawIOXml(cells);
}

function buildDrawIOXml(cells: string[]): string {
  return `<?xml version="1.0" encoding="UTF-8"?>
<mxfile host="app.diagrams.net" version="21.0.0" type="device">
  <diagram name="Trace Diagram" id="trace-diagram">
    <mxGraphModel dx="0" dy="0" grid="1" gridSize="10" guides="1" tooltips="1" connect="1" arrows="1" fold="1" page="1" pageScale="1" pageWidth="827" pageHeight="1169">
      <root>
        <mxCell id="0"/>
        <mxCell id="1" parent="0"/>
${cells.join("\n")}
      </root>
    </mxGraphModel>
  </diagram>
</mxfile>`;
}
