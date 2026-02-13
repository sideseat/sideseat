import type { TreeNode } from "./types";

export function getTraceTimeRange(root: TreeNode): {
  start: Date;
  duration: number;
} {
  const resolveEnd = (node: TreeNode): Date => {
    if (node.endTime) return node.endTime;
    if (node.duration !== undefined) {
      return new Date(node.startTime.getTime() + Math.max(0, node.duration));
    }
    return node.startTime;
  };

  let minStart = root.startTime;
  let maxEnd = resolveEnd(root);

  function traverse(node: TreeNode) {
    if (node.startTime < minStart) minStart = node.startTime;
    const nodeEnd = resolveEnd(node);
    if (nodeEnd > maxEnd) maxEnd = nodeEnd;
    for (const child of node.children) {
      traverse(child);
    }
  }

  traverse(root);

  return {
    start: minStart,
    duration: maxEnd.getTime() - minStart.getTime(),
  };
}
