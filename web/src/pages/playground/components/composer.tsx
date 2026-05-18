/* Adapted from engagement-mck/solution/site/src/components/chat/composer.tsx */
import { ArrowUp, Square } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

interface Props {
  onSend: (prompt: string) => void;
  onCancel: () => void;
  isStreaming: boolean;
  disabled: boolean;
  placeholder: string;
  /** Bumped by the parent to refocus the textarea on agent select. */
  focusKey?: number;
  /** Initial visible rows. Defaults to 1 (chat); landing uses 4. */
  rows?: number;
}

export function Composer({
  onSend,
  onCancel,
  isStreaming,
  disabled,
  placeholder,
  focusKey,
  rows = 1,
}: Props) {
  const [value, setValue] = useState("");
  const [focused, setFocused] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // Refocus on parent-driven focus bump (e.g., agent picked).
  useEffect(() => {
    if (focusKey !== undefined && !disabled) textareaRef.current?.focus();
  }, [focusKey, disabled]);

  // Manual autosize: cap at 240px so a giant paste can't push the
  // viewport's scroll content off-screen. The minimum height honours the
  // requested `rows` so the landing-mode textarea reads as multiline
  // even before the user types.
  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    const lineHeight = parseFloat(getComputedStyle(el).lineHeight) || 20;
    const verticalPadding = 12; // py-1.5 → 6px top + 6px bottom
    const minHeight = rows * lineHeight + verticalPadding;
    el.style.height = `${Math.min(Math.max(el.scrollHeight, minHeight), 240)}px`;
  }, [value, rows]);

  const send = useCallback(() => {
    const trimmed = value.trim();
    if (!trimmed || disabled || isStreaming) return;
    onSend(trimmed);
    setValue("");
  }, [value, disabled, isStreaming, onSend]);

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  };

  const canSend = !isStreaming && !disabled && value.trim().length > 0;

  return (
    <div
      onClick={() => textareaRef.current?.focus()}
      className={cn(
        "flex min-h-11 cursor-text items-end gap-2 rounded-xl border bg-card px-1.5 py-1.5 shadow-sm transition-[border-color,box-shadow] duration-150",
        focused && "border-foreground/30 ring-[3px] ring-ring/20",
      )}
    >
      <textarea
        ref={textareaRef}
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={onKeyDown}
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
        placeholder={placeholder}
        disabled={disabled}
        rows={rows}
        // Inline `box-shadow: none` overrides the theme-level
        // `textarea:focus { box-shadow: ... }` rule that themes inject.
        // The outer wrapper owns the focus ring; the textarea must not
        // draw its own.
        style={{ boxShadow: "none" }}
        className="min-h-8 flex-1 resize-none border-0 bg-transparent px-2 py-1.5 text-sm leading-5 outline-none ring-0 focus:outline-none focus:ring-0 focus-visible:outline-none focus-visible:ring-0 placeholder:text-muted-foreground disabled:cursor-not-allowed disabled:opacity-50"
      />
      {isStreaming ? (
        <Button
          type="button"
          variant="destructive"
          size="icon"
          className="size-8 shrink-0"
          onClick={(e) => {
            e.stopPropagation();
            onCancel();
          }}
          aria-label="Stop"
        >
          <Square className="size-3.5 fill-current" />
        </Button>
      ) : (
        <Button
          type="button"
          size="icon"
          className="size-8 shrink-0"
          onClick={(e) => {
            e.stopPropagation();
            send();
          }}
          disabled={!canSend}
          aria-label="Send"
        >
          <ArrowUp className="size-4" />
        </Button>
      )}
    </div>
  );
}
