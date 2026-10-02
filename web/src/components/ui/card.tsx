import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";

import { cn } from "@/lib/utils";

const cardVariants = cva("text-card-foreground flex flex-col rounded-xl border shadow-sm", {
  variants: {
    variant: {
      default: "bg-card",
      // Dashboard tiles that sit on a tinted page background.
      translucent: "border-border/60 bg-card/80",
    },
    size: {
      default: "gap-6 py-6",
      sm: "gap-3 py-4",
    },
    // Cards that act as a link or a selectable option.
    interactive: {
      true: "transition-colors hover:border-primary/50 hover:bg-accent/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/60",
      false: "",
    },
    selected: {
      true: "border-primary/60 ring-1 ring-primary/40",
      false: "",
    },
  },
  compoundVariants: [
    {
      variant: "translucent",
      interactive: true,
      className: "transition-all hover:border-primary/30 hover:bg-background/80 hover:shadow-md",
    },
  ],
  defaultVariants: {
    variant: "default",
    size: "default",
    interactive: false,
    selected: false,
  },
});

function Card({
  className,
  variant,
  size,
  interactive,
  selected,
  ...props
}: React.ComponentProps<"div"> & VariantProps<typeof cardVariants>) {
  return (
    <div
      data-slot="card"
      className={cn(cardVariants({ variant, size, interactive, selected }), className)}
      {...props}
    />
  );
}

const cardHeaderVariants = cva(
  "@container/card-header grid auto-rows-min grid-rows-[auto_auto] items-start gap-2 px-6 has-data-[slot=card-action]:grid-cols-[1fr_auto] [.border-b]:pb-6",
  {
    variants: {
      size: {
        default: "",
        // Pairs with CardTitle size="sm" for dense dashboard widgets.
        sm: "pb-2",
      },
    },
    defaultVariants: { size: "default" },
  },
);

function CardHeader({
  className,
  size,
  ...props
}: React.ComponentProps<"div"> & VariantProps<typeof cardHeaderVariants>) {
  return (
    <div
      data-slot="card-header"
      className={cn(cardHeaderVariants({ size }), className)}
      {...props}
    />
  );
}

const cardTitleVariants = cva("gap-2", {
  variants: {
    size: {
      sm: "text-sm leading-none font-medium",
      default: "leading-none font-semibold",
      md: "text-base leading-tight font-semibold",
      lg: "text-xl leading-tight font-semibold",
      xl: "text-2xl leading-none font-bold",
    },
  },
  defaultVariants: { size: "default" },
});

function CardTitle({
  className,
  size,
  ...props
}: React.ComponentProps<"div"> & VariantProps<typeof cardTitleVariants>) {
  return (
    <div data-slot="card-title" className={cn(cardTitleVariants({ size }), className)} {...props} />
  );
}

function CardDescription({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="card-description"
      className={cn("text-muted-foreground text-sm", className)}
      {...props}
    />
  );
}

function CardAction({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="card-action"
      className={cn("col-start-2 row-span-2 row-start-1 self-start justify-self-end", className)}
      {...props}
    />
  );
}

function CardContent({ className, ...props }: React.ComponentProps<"div">) {
  return <div data-slot="card-content" className={cn("px-6", className)} {...props} />;
}

function CardFooter({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="card-footer"
      className={cn("flex items-center px-6 [.border-t]:pt-6", className)}
      {...props}
    />
  );
}

export { Card, CardHeader, CardFooter, CardTitle, CardAction, CardDescription, CardContent };
