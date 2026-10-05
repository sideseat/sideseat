import { useCallback } from "react";
import JsonView from "@uiw/react-json-view";
import { cn } from "@/lib/utils";
import { highlightText, MAX_SEARCH_LENGTH } from "./highlight-text";

interface JsonContentProps {
  data: unknown;
  collapsed?: number | boolean;
  disableCollapse?: boolean;
  highlight?: string;
}

const HiddenArrow = () => null;

export function JsonContent({
  data,
  collapsed = false,
  disableCollapse = false,
  highlight = "",
}: JsonContentProps) {
  const trimmedHighlight = highlight.trim();
  const searchTerm = trimmedHighlight.length <= MAX_SEARCH_LENGTH ? trimmedHighlight : "";

  const renderValue = useCallback(
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    ({ children, ...props }: any) => {
      if (!searchTerm || children == null)
        return <span {...props}>{children as React.ReactNode}</span>;
      // Convert to string for highlighting (handles numbers, booleans, etc.)
      const text = typeof children === "string" ? children : String(children);
      return <span {...props}>{highlightText(text, searchTerm)}</span>;
    },
    [searchTerm],
  );

  // The tree viewer only understands objects and arrays: given a bare string it lists the characters as
  // indexed keys, given a number it prints `{}`, and given null it throws and takes the whole thread down.
  // Tool inputs, tool results and JSON blocks are arbitrary JSON, so scalars are printed as JSON text.
  if (data === null || typeof data !== "object") {
    const text = typeof data === "string" ? data : (JSON.stringify(data) ?? String(data));
    return (
      <pre className="json-viewer overflow-x-auto font-mono text-sm break-all whitespace-pre-wrap">
        {searchTerm ? highlightText(text, searchTerm) : text}
      </pre>
    );
  }

  return (
    <div className={cn("overflow-x-auto", disableCollapse && "json-no-collapse")}>
      <JsonView
        key={searchTerm}
        value={data}
        displayDataTypes={false}
        displayObjectSize={false}
        collapsed={collapsed}
        shortenTextAfterLength={0}
        // The viewer sets font-size inline, so the size class must be important to win.
        className="json-viewer break-all text-sm!"
      >
        {disableCollapse && <JsonView.Arrow render={HiddenArrow} />}
        {searchTerm && (
          <>
            <JsonView.String render={renderValue} />
            <JsonView.KeyName render={renderValue} />
            <JsonView.Int render={renderValue} />
            <JsonView.Float render={renderValue} />
            <JsonView.True render={renderValue} />
            <JsonView.False render={renderValue} />
            <JsonView.Null render={renderValue} />
          </>
        )}
      </JsonView>
    </div>
  );
}
