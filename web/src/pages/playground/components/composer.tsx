import { Send, Square } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";

interface Props {
  onSend: (prompt: string) => void;
  onCancel: () => void;
  isStreaming: boolean;
  disabled: boolean;
  placeholder: string;
  /** Bumped by parent to refocus textarea (e.g. after agent select). */
  focusKey?: number;
}

export function Composer({
  onSend,
  onCancel,
  isStreaming,
  disabled,
  placeholder,
  focusKey,
}: Props) {
  const ref = useRef<HTMLTextAreaElement>(null);
  const [value, setValue] = useState("");

  useEffect(() => {
    if (focusKey !== undefined && !disabled) ref.current?.focus();
  }, [focusKey, disabled]);

  const submit = () => {
    if (disabled || isStreaming) return;
    const trimmed = value.trim();
    if (!trimmed) return;
    onSend(trimmed);
    setValue("");
    ref.current?.focus();
  };

  return (
    <div className="my-3 rounded-md border bg-card p-2">
      <div className="flex items-end gap-2">
        <textarea
          ref={ref}
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              submit();
            }
          }}
          placeholder={placeholder}
          disabled={disabled}
          rows={1}
          className="field-sizing-content flex-1 resize-none bg-transparent px-2 py-1.5 text-sm outline-none placeholder:text-muted-foreground disabled:opacity-50 max-h-48"
        />
        {isStreaming ? (
          <Button
            type="button"
            size="icon"
            variant="destructive"
            onClick={onCancel}
            aria-label="Stop"
          >
            <Square className="size-4" />
          </Button>
        ) : (
          <Button
            type="button"
            size="icon"
            onClick={submit}
            disabled={disabled || value.trim().length === 0}
            aria-label="Send"
          >
            <Send className="size-4" />
          </Button>
        )}
      </div>
    </div>
  );
}
