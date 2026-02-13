import { useEffect, useMemo, useState, type ChangeEvent, type KeyboardEvent } from "react";
import { ChevronFirst, ChevronLast, ChevronLeft, ChevronRight, Loader2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

type PageChangeHandler = (page: number) => void;
type PageSizeChangeHandler = (size: number) => void;

interface PaginationProps {
  currentPage: number;
  pageSize: number;
  totalItems?: number;
  totalPages?: number;
  onPageChange: PageChangeHandler;
  onPageSizeChange: PageSizeChangeHandler;
  pageSizeOptions?: number[];
  showTotal?: boolean;
  showFirstLast?: boolean;
  isLoading?: boolean;
  disabled?: boolean;
  className?: string;
  selectedCount?: number;
}

const DEFAULT_PAGE_SIZES = [10, 20, 50, 100];
export const DEFAULT_PAGE_SIZE = 20;

export function Pagination({
  currentPage,
  pageSize,
  totalItems,
  totalPages,
  onPageChange,
  onPageSizeChange,
  pageSizeOptions = DEFAULT_PAGE_SIZES,
  showTotal = true,
  showFirstLast = true,
  isLoading = false,
  disabled = false,
  className,
  selectedCount,
}: PaginationProps) {
  const derivedTotalPages = useMemo(() => {
    if (typeof totalPages === "number" && totalPages > 0) return totalPages;
    if (typeof totalItems === "number" && totalItems > 0) {
      return Math.max(1, Math.ceil(totalItems / pageSize));
    }
    return undefined;
  }, [totalItems, totalPages, pageSize]);

  const [pageInput, setPageInput] = useState(String(currentPage));

  useEffect(() => {
    setPageInput(String(currentPage));
  }, [currentPage]);

  const isDisabled = disabled || isLoading;
  const canGoPrevious = currentPage > 1;
  const canGoNext = derivedTotalPages ? currentPage < derivedTotalPages : true;

  const clampPage = (page: number) => {
    if (page < 1) return 1;
    if (derivedTotalPages) return Math.min(page, derivedTotalPages);
    return page;
  };

  const handlePageInputChange = (e: ChangeEvent<HTMLInputElement>) => {
    setPageInput(e.target.value.replace(/[^\d]/g, ""));
  };

  const commitPageInput = () => {
    const parsed = parseInt(pageInput, 10);
    if (!isNaN(parsed) && parsed >= 1) {
      onPageChange(clampPage(parsed));
    } else {
      setPageInput(String(currentPage));
    }
  };

  const handlePageInputKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") {
      commitPageInput();
    } else if (e.key === "Escape") {
      setPageInput(String(currentPage));
      (e.target as HTMLInputElement).blur();
    }
  };

  const handlePageInputBlur = () => {
    commitPageInput();
  };

  const handlePageSizeChange = (value: string) => {
    const nextSize = Number(value);
    if (Number.isFinite(nextSize) && nextSize > 0) {
      const nextTotalPages =
        typeof totalItems === "number" && totalItems > 0
          ? Math.max(1, Math.ceil(totalItems / nextSize))
          : undefined;

      const nextPage = nextTotalPages != null ? Math.min(currentPage, nextTotalPages) : 1;

      if (nextPage !== currentPage) {
        onPageChange(nextPage);
      }

      onPageSizeChange(nextSize);
    }
  };

  const goToPage = (page: number) => onPageChange(clampPage(page));

  const displayTotalPages = derivedTotalPages ?? currentPage;

  const formatNumber = (num: number | undefined) => {
    if (num === undefined) return "—";
    return num.toLocaleString();
  };

  return (
    <div
      data-slot="pagination"
      className={cn(
        "flex w-full min-w-0 items-center gap-2 whitespace-nowrap overflow-hidden rounded-lg border px-3 py-2 sm:gap-3",
        className,
      )}
    >
      <div className="flex items-center gap-2 text-sm text-muted-foreground sm:gap-3">
        <div className="flex items-center gap-2">
          <span className="hidden min-[900px]:inline">Rows per page</span>
          <Select
            value={String(pageSize)}
            onValueChange={handlePageSizeChange}
            disabled={isDisabled}
          >
            <SelectTrigger className="h-8 w-24">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {pageSizeOptions.map((size) => (
                <SelectItem key={size} value={String(size)}>
                  {size}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        {showTotal && (
          <span className="hidden min-[900px]:inline">{formatNumber(totalItems)} total</span>
        )}

        {selectedCount != null && selectedCount > 0 && (
          <span className="rounded-md bg-primary/10 px-2 py-0.5 text-xs font-medium text-primary">
            {selectedCount} selected
          </span>
        )}

        {isLoading && <Loader2 className="size-4 animate-spin" aria-label="Loading page" />}
      </div>

      <div className="ml-auto flex items-center gap-1 text-sm text-muted-foreground sm:gap-2">
        <div className="flex items-center gap-2">
          <span className="hidden min-[900px]:inline text-xs font-medium text-muted-foreground sm:text-sm sm:font-normal">
            Page
          </span>
          <Input
            type="text"
            inputMode="numeric"
            name="current-page"
            value={pageInput}
            onChange={handlePageInputChange}
            onKeyDown={handlePageInputKeyDown}
            onBlur={handlePageInputBlur}
            disabled={isDisabled}
            className="h-8 w-[86px] text-center sm:w-20"
            aria-label="Current page"
            maxLength={6}
          />
          <span className="hidden min-[900px]:inline">of {formatNumber(displayTotalPages)}</span>
        </div>

        <div className="flex items-center gap-1">
          {showFirstLast && (
            <Button
              variant="outline"
              size="icon-sm"
              onClick={() => goToPage(1)}
              disabled={!canGoPrevious || isDisabled}
              title="First page"
            >
              <ChevronFirst className="size-4" />
            </Button>
          )}
          <Button
            variant="outline"
            size="icon-sm"
            onClick={() => goToPage(currentPage - 1)}
            disabled={!canGoPrevious || isDisabled}
            title="Previous page"
          >
            <ChevronLeft className="size-4" />
          </Button>
          <Button
            variant="outline"
            size="icon-sm"
            onClick={() => goToPage(currentPage + 1)}
            disabled={!canGoNext || isDisabled}
            title="Next page"
          >
            <ChevronRight className="size-4" />
          </Button>
          {showFirstLast && (
            <Button
              variant="outline"
              size="icon-sm"
              onClick={() => derivedTotalPages && goToPage(derivedTotalPages)}
              disabled={!derivedTotalPages || !canGoNext || isDisabled}
              title="Last page"
            >
              <ChevronLast className="size-4" />
            </Button>
          )}
        </div>
      </div>
    </div>
  );
}
