import { ArrowDown } from "lucide-react";
import { Button } from "@/components/ui/button";

interface Props {
  onClick: () => void;
}

export function ScrollToBottom({ onClick }: Props) {
  return (
    <Button
      type="button"
      size="sm"
      variant="secondary"
      onClick={onClick}
      className="pointer-events-auto absolute bottom-4 left-1/2 -translate-x-1/2 shadow-md"
    >
      <ArrowDown className="size-4" />
      Latest
    </Button>
  );
}
