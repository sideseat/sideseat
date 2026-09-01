import type { ICellRendererParams } from "ag-grid-community";
import { formatTokens } from "@/lib/format";
import type { TokenBreakdown } from "@/components/breakdown-popover";
import { UsageBreakdownPopover } from "@/components/breakdown-popover";
import { tokenDisplay } from "@/lib/token-breakdown";

export function TokensCellRenderer(params: ICellRendererParams<TokenBreakdown>) {
  const data = params.data;

  if (!data) return null;

  const { input, output, total } = tokenDisplay(data);

  if (total === 0) return <span className="text-muted-foreground">-</span>;

  return (
    <UsageBreakdownPopover data={data}>
      <span className="w-full h-full flex items-center tabular-nums">
        {formatTokens(input)} &rarr; {formatTokens(output)} (&Sigma; {formatTokens(total)})
      </span>
    </UsageBreakdownPopover>
  );
}
