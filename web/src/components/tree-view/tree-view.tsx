import { cn } from "@/lib/utils";
import { TreeNode } from "./tree-node";
import type { TreeNodeData, TreeViewProps } from "./types";

export function TreeView<T extends TreeNodeData>({
  data,
  selectedId,
  onSelect,
  collapsedIds,
  onToggleCollapse,
  renderContent,
  getRowClassName,
  className,
}: TreeViewProps<T>) {
  if (!data) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
        No data available
      </div>
    );
  }

  // For virtual roots (multi-trace sessions), render children as separate trees
  const isVirtualRoot = "isVirtualRoot" in data && data.isVirtualRoot === true;
  const rootNodes = isVirtualRoot && data.children ? (data.children as T[]) : [data];

  return (
    <div className={cn("h-full overflow-auto", className)}>
      <div role="tree" className="w-max min-w-full p-3">
        {rootNodes.map((node, index) => (
          <TreeNode
            key={node.id}
            node={node}
            depth={0}
            isLast={index === rootNodes.length - 1}
            selectedId={selectedId}
            onSelect={onSelect}
            collapsedIds={collapsedIds}
            onToggleCollapse={onToggleCollapse}
            renderContent={renderContent}
            getRowClassName={getRowClassName}
          />
        ))}
      </div>
    </div>
  );
}
