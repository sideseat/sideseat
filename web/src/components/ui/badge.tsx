import * as React from "react";
import { Slot } from "@radix-ui/react-slot";
import { cva, type VariantProps } from "class-variance-authority";

import { cn } from "@/lib/utils";

const badgeVariants = cva(
  "inline-flex items-center justify-center rounded-full border px-2 py-0.5 text-xs font-medium w-fit whitespace-nowrap shrink-0 [&>svg]:size-3 gap-1 [&>svg]:pointer-events-none focus-visible:border-ring focus-visible:ring-ring/50 focus-visible:ring-[3px] aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive transition-[color,box-shadow] overflow-hidden",
  {
    variants: {
      variant: {
        default: "border-transparent bg-primary text-primary-foreground [a&]:hover:bg-primary/90",
        secondary:
          "border-transparent bg-secondary text-secondary-foreground [a&]:hover:bg-secondary/90",
        destructive:
          "border-transparent bg-destructive text-white [a&]:hover:bg-destructive/90 focus-visible:ring-destructive/20 dark:focus-visible:ring-destructive/40 dark:bg-destructive/60",
        outline: "text-foreground [a&]:hover:bg-accent [a&]:hover:text-accent-foreground",
        success: "border-success/30 bg-success/10 text-success-foreground",
      },
      size: {
        default: "",
        // Dense metadata such as status pills and source markers.
        sm: "px-1.5 text-3xs",
      },
      weight: {
        default: "",
        normal: "font-normal",
      },
      // Values that read better aligned: durations, token counts, model ids.
      mono: {
        true: "font-mono",
        false: "",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
      weight: "default",
      mono: false,
    },
  },
);

function Badge({
  className,
  variant,
  size,
  weight,
  mono,
  asChild = false,
  ...props
}: React.ComponentProps<"span"> & VariantProps<typeof badgeVariants> & { asChild?: boolean }) {
  const Comp = asChild ? Slot : "span";

  return (
    <Comp
      data-slot="badge"
      className={cn(badgeVariants({ variant, size, weight, mono }), className)}
      {...props}
    />
  );
}

export { Badge, badgeVariants };
