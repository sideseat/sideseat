import { useMemo, useCallback } from "react";
import JsonView from "@uiw/react-json-view";
import { lightTheme } from "@uiw/react-json-view/light";
import { vscodeTheme } from "@uiw/react-json-view/vscode";
import { useTheme } from "@/components/theme-provider";
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
  const { resolvedTheme } = useTheme();
  const isDark = resolvedTheme === "dark";
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

  const jsonStyle = useMemo(
    () => ({
      ...(isDark ? vscodeTheme : lightTheme),
      backgroundColor: "transparent",
      fontFamily: "inherit",
      fontSize: "0.875rem",
      wordBreak: "break-all" as const,
    }),
    [isDark],
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
        style={jsonStyle}
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
