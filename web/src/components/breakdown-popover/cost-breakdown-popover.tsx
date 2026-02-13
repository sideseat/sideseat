import type { ReactNode } from "react";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { formatCost } from "@/lib/format";
import { useHoverPopover } from "@/hooks/use-hover-popover";
import { BreakdownRow } from "./breakdown-row";

export interface CostBreakdown {
  input_cost: number;
  output_cost: number;
  cache_read_cost: number;
  cache_write_cost: number;
  reasoning_cost: number;
  total_cost: number;
}

interface CostBreakdownPopoverProps {
  data: CostBreakdown;
  children: ReactNode;
}

export function CostBreakdownPopover({ data, children }: CostBreakdownPopoverProps) {
  const { open, setOpen, handleMouseEnter, handleMouseLeave } = useHoverPopover();

  const { input_cost, output_cost, cache_read_cost, cache_write_cost, reasoning_cost, total_cost } =
    data;

  const inputTotal = input_cost + cache_read_cost + cache_write_cost;
  const outputTotal = output_cost + reasoning_cost;
  const grandTotal = total_cost || inputTotal + outputTotal;

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <span
          className="cursor-default"
          onMouseEnter={handleMouseEnter}
          onMouseLeave={handleMouseLeave}
        >
          {children}
        </span>
      </PopoverTrigger>
      <PopoverContent
        className="w-72"
        align="center"
        side="top"
        onMouseEnter={handleMouseEnter}
        onMouseLeave={handleMouseLeave}
      >
        <div className="space-y-3">
          <h4 className="font-semibold">Cost Breakdown</h4>
          <div className="space-y-2 text-sm">
            <div className="space-y-1">
              <BreakdownRow label="Input" value={formatCost(inputTotal)} bold />
              <BreakdownRow label="input" value={formatCost(input_cost)} indent />
              <BreakdownRow label="cache read" value={formatCost(cache_read_cost)} indent />
              <BreakdownRow label="cache write" value={formatCost(cache_write_cost)} indent />
            </div>
            <div className="space-y-1">
              <BreakdownRow label="Output" value={formatCost(outputTotal)} bold />
              <BreakdownRow label="output" value={formatCost(output_cost)} indent />
              {reasoning_cost > 0 && (
                <BreakdownRow label="reasoning" value={formatCost(reasoning_cost)} indent />
              )}
            </div>
            <div className="border-t pt-2">
              <BreakdownRow label="Total" value={formatCost(grandTotal)} bold />
            </div>
          </div>
          <p className="text-[10px] text-muted-foreground mt-3">
            Estimates based on published pricing. Actual costs may vary.
          </p>
        </div>
      </PopoverContent>
    </Popover>
  );
}
