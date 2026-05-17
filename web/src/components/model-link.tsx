import { ExternalLink } from "lucide-react";
import { cloudPriceModelUrl } from "@/lib/utils";
import { cn } from "@/lib/utils";

interface ModelLinkProps {
  model: string;
  className?: string;
  showIcon?: boolean;
}

export function ModelLink({ model, className, showIcon = true }: ModelLinkProps) {
  return (
    <a
      href={cloudPriceModelUrl(model)}
      target="_blank"
      rel="noopener noreferrer"
      title={`Open ${model} on CloudPrice`}
      className={cn(
        "inline-flex items-center gap-1 text-primary underline decoration-dotted underline-offset-2 hover:decoration-solid",
        className,
      )}
      onClick={(e) => e.stopPropagation()}
    >
      <span className="truncate">{model}</span>
      {showIcon && <ExternalLink className="h-3 w-3 shrink-0 opacity-70" />}
    </a>
  );
}
