import { ChevronDown } from "lucide-react";
import { cn } from "@/lib/utils";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import type { TreeNodeData, TreeNodeProps } from "./types";

export function TreeNode<T extends TreeNodeData>({
  node,
  depth,
  isLast,
  selectedId,
  onSelect,
  collapsedIds,
  onToggleCollapse,
  renderContent,
  getRowClassName,
}: TreeNodeProps<T>) {
  const isSelected = selectedId === node.id;
  const isCollapsed = collapsedIds?.has(node.id) ?? false;
  const hasChildren = (node.children?.length ?? 0) > 0;
  const isRoot = depth === 0;

  const state = {
    isSelected,
    isCollapsed,
    hasChildren,
    depth,
    isLast,
  };

  return (
    <div className="relative">
      {/* Vertical line from parent (not for root) */}
      {!isRoot && (
        <div className={cn("absolute left-0 top-0 w-px bg-border", isLast ? "h-5" : "h-full")} />
      )}

      {/* Horizontal connector (not for root) */}
      {!isRoot && <div className="absolute left-0 top-5 w-4 h-px bg-border" />}

      <div className={cn(!isRoot && "ml-4")}>
        <Collapsible open={!isCollapsed}>
          {/* Node row */}
          <div
            role="treeitem"
            aria-selected={isSelected}
            aria-expanded={hasChildren ? !isCollapsed : undefined}
            className={cn(
              "group flex items-center gap-2 rounded-md px-2 py-1.5 cursor-pointer transition-colors",
              "hover:bg-accent-foreground/10",
              isSelected && "bg-accent-foreground/5",
              getRowClassName?.(node),
            )}
            onClick={() => onSelect?.(node.id)}
          >
            {/* Custom content */}
            <div className="flex min-w-0 flex-1">{renderContent(node, state)}</div>

            {/* Collapse button - RIGHT side */}
            {hasChildren && (
              <CollapsibleTrigger asChild>
                <button
                  type="button"
                  className="h-5 w-5 shrink-0 flex items-center justify-center rounded hover:bg-accent"
                  onClick={(e) => {
                    e.stopPropagation();
                    onToggleCollapse?.(node.id);
                  }}
                  aria-label={isCollapsed ? "Expand" : "Collapse"}
                >
                  <ChevronDown
                    className={cn("h-3.5 w-3.5 transition-transform", isCollapsed && "-rotate-90")}
                  />
                </button>
              </CollapsibleTrigger>
            )}
          </div>

          {/* Children - recursive */}
          {hasChildren && node.children && (
            <CollapsibleContent className="pl-2">
              {node.children.map((child, index, arr) => (
                <TreeNode
                  key={child.id}
                  node={child as T}
                  depth={depth + 1}
                  isLast={index === arr.length - 1}
                  selectedId={selectedId}
                  onSelect={onSelect}
                  collapsedIds={collapsedIds}
                  onToggleCollapse={onToggleCollapse}
                  renderContent={renderContent}
                  getRowClassName={getRowClassName}
                />
              ))}
            </CollapsibleContent>
          )}
        </Collapsible>
      </div>
    </div>
  );
}
