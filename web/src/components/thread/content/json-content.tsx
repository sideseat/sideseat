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

  return (
    <div className={cn("overflow-x-auto", disableCollapse && "json-no-collapse")}>
      <JsonView
        key={searchTerm}
        value={data as object}
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
