interface BreakdownRowProps {
  label: string;
  value: string;
  indent?: boolean;
  bold?: boolean;
}

export function BreakdownRow({ label, value, indent, bold }: BreakdownRowProps) {
  return (
    <div className="flex justify-between gap-4">
      <span className={indent ? "text-muted-foreground" : ""}>{label}</span>
      <span className={bold ? "font-semibold tabular-nums" : "tabular-nums"}>{value}</span>
    </div>
  );
}
