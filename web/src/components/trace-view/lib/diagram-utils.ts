import ELK from "elkjs/lib/elk.bundled.js";
import type { Node, Edge } from "@xyflow/react";
import type { TreeNode } from "./types";

const elk = new ELK();

export const NODE_WIDTH = 200;
export const NODE_HEIGHT = 70;

export function treeToFlow(tree: TreeNode): { nodes: Node[]; edges: Edge[] } {
  const nodes: Node[] = [];
  const edges: Edge[] = [];

  function traverse(node: TreeNode, parentId: string | null) {
    // Skip virtual root - traverse children directly without creating a node
    if (node.isVirtualRoot) {
      for (const child of node.children) {
        traverse(child, null);
      }
      return;
    }

    nodes.push({
      id: node.id,
      type: "span",
      position: { x: 0, y: 0 },
      data: {
        label: node.name,
        type: node.type,
        duration: node.duration,
        tokens: node.totalTokens,
        cost: node.totalCost,
        startTime: node.startTime.getTime(),
        isError: node.span.status_code === "ERROR",
      },
    });

    // Add edge from parent if exists
    if (parentId) {
      edges.push({
        id: `${parentId}-${node.id}`,
        source: parentId,
        target: node.id,
        type: "default",
      });
    }

    // Children are already sorted by startTime in buildTree()
    for (const child of node.children) {
      traverse(child, node.id);
    }
  }

  traverse(tree, null);
  return { nodes, edges };
}

export async function layoutNodes({
  nodes,
  edges,
}: {
  nodes: Node[];
  edges: Edge[];
}): Promise<{ nodes: Node[]; edges: Edge[] }> {
  const elkGraph = {
    id: "root",
    layoutOptions: {
      "elk.algorithm": "layered",
      "elk.direction": "RIGHT",
      "elk.spacing.nodeNode": "80",
      "elk.layered.spacing.nodeNodeBetweenLayers": "150",
      "elk.layered.nodePlacement.strategy": "NETWORK_SIMPLEX",
      "elk.layered.crossingMinimization.strategy": "LAYER_SWEEP",
      "elk.layered.considerModelOrder.strategy": "NODES_AND_EDGES",
    },
    children: nodes.map((node) => ({
      id: node.id,
      width: NODE_WIDTH,
      height: NODE_HEIGHT,
    })),
    edges: edges.map((edge) => ({
      id: edge.id,
      sources: [edge.source],
      targets: [edge.target],
    })),
  };

  const layouted = await elk.layout(elkGraph);

  const positionMap = new Map(
    layouted.children?.map((n) => [n.id, { x: n.x ?? 0, y: n.y ?? 0 }]) ?? [],
  );

  const layoutedNodes = nodes.map((node) => ({
    ...node,
    position: positionMap.get(node.id) ?? { x: 0, y: 0 },
  }));

  return { nodes: layoutedNodes, edges };
}
