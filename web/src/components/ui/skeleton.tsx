import { cva, type VariantProps } from "class-variance-authority";

import { cn } from "@/lib/utils";

const skeletonVariants = cva("bg-accent animate-pulse", {
  variants: {
    // Match the radius of the element the skeleton stands in for.
    shape: {
      default: "rounded-md",
      lg: "rounded-lg",
      xl: "rounded-xl",
      circle: "rounded-full",
    },
  },
  defaultVariants: { shape: "default" },
});

function Skeleton({
  className,
  shape,
  ...props
}: React.ComponentProps<"div"> & VariantProps<typeof skeletonVariants>) {
  return (
    <div data-slot="skeleton" className={cn(skeletonVariants({ shape }), className)} {...props} />
  );
}

export { Skeleton };
