import { Badge } from "@/components/ui/badge";

/** Marks a tool the provider ran itself, inside its response: the application never executed it. */
export function ProviderRunBadge() {
  return (
    <Badge
      variant="secondary"
      size="sm"
      title="The model provider ran this tool inside its response"
    >
      run by provider
    </Badge>
  );
}
