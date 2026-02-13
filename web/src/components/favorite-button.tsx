import { Star } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

interface FavoriteButtonProps {
  isFavorite: boolean;
  disabled?: boolean;
  onToggle: () => void;
  className?: string;
}

/**
 * Star button for toggling favorites.
 * When favorited, the star is filled yellow.
 * Relies on optimistic updates for instant visual feedback.
 */
export function FavoriteButton({
  isFavorite,
  disabled = false,
  onToggle,
  className,
}: FavoriteButtonProps) {
  return (
    <Button
      variant="ghost"
      size="icon-sm"
      className={cn("h-7 w-7", className)}
      disabled={disabled}
      onClick={onToggle}
      aria-label={isFavorite ? "Remove from favorites" : "Add to favorites"}
    >
      <Star
        className={cn(
          "h-3.5 w-3.5 transition-colors",
          isFavorite && "fill-yellow-400 text-yellow-400",
        )}
      />
    </Button>
  );
}
