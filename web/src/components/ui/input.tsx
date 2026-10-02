import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";

import { cn } from "@/lib/utils";

const inputVariants = cva(
  [
    "file:text-foreground placeholder:text-muted-foreground selection:bg-primary selection:text-primary-foreground dark:bg-input/30 border-input w-full min-w-0 rounded-md border bg-transparent px-3 py-1 shadow-xs transition-[color,box-shadow] outline-none file:inline-flex file:h-7 file:border-0 file:bg-transparent file:text-sm file:font-medium disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50",
    "focus-visible:border-ring focus-visible:ring-ring/50 focus-visible:ring-[3px]",
    "aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive",
  ],
  {
    variants: {
      variant: {
        default: "",
        // Two-digit time fields (hh, mm, ss) whose digits must line up.
        segment: "px-1 tabular-nums",
        // A stored value shown for reference only, such as a masked secret.
        readonly: "cursor-default opacity-60 focus-visible:ring-0",
      },
      // Named inputSize because `size` is a native input attribute.
      inputSize: {
        default: "h-9 text-base md:text-sm",
        sm: "h-8 text-xs",
        xs: "h-7 text-xs",
      },
      // Room for controls drawn over the input: a leading search icon, a
      // trailing clear or reveal button, or both.
      adornment: {
        none: "",
        start: "pl-9",
        end: "pr-10",
        both: "pl-9 pr-10",
      },
    },
    compoundVariants: [
      { inputSize: ["sm", "xs"], adornment: "start", className: "pl-7" },
      { inputSize: ["sm", "xs"], adornment: "both", className: "pl-7 pr-7" },
    ],
    defaultVariants: { variant: "default", inputSize: "default", adornment: "none" },
  },
);

function Input({
  className,
  type,
  variant,
  inputSize,
  adornment,
  ...props
}: React.ComponentProps<"input"> & VariantProps<typeof inputVariants>) {
  return (
    <input
      type={type}
      data-slot="input"
      className={cn(inputVariants({ variant, inputSize, adornment }), className)}
      {...props}
    />
  );
}

export { Input };
