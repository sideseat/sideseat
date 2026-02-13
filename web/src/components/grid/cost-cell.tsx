import type { ICellRendererParams } from "ag-grid-community";
import { formatCost } from "@/lib/format";
import type { CostBreakdown } from "@/components/breakdown-popover";
import { CostBreakdownPopover } from "@/components/breakdown-popover";

export function CostCellRenderer(params: ICellRendererParams<CostBreakdown>) {
  const data = params.data;

  if (!data) return null;

  const { total_cost } = data;

  return (
    <CostBreakdownPopover data={data}>
      <span className="w-full h-full flex items-center tabular-nums">{formatCost(total_cost)}</span>
    </CostBreakdownPopover>
  );
}
