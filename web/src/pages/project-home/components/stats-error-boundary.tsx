import { Component, type ReactNode } from "react";
import { AlertTriangle, RefreshCw } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";

/**
 * Fallback component for when stats query fails.
 * Use this with React Query's error state.
 */
export function StatsError({ onRetry }: { onRetry?: () => void }) {
  return (
    <Card>
      <CardContent className="py-8">
        <div className="flex flex-col items-center justify-center text-center">
          <AlertTriangle className="h-8 w-8 text-muted-foreground mb-3" />
          <h3 className="font-medium mb-1">Failed to load stats</h3>
          <p className="text-sm text-muted-foreground mb-4">
            There was an error loading the dashboard data.
          </p>
          {onRetry && (
            <Button variant="outline" size="sm" onClick={onRetry}>
              <RefreshCw className="mr-2 h-4 w-4" />
              Retry
            </Button>
          )}
        </div>
      </CardContent>
    </Card>
  );
}

interface WidgetErrorBoundaryProps {
  children: ReactNode;
  title?: string;
}

interface WidgetErrorBoundaryState {
  hasError: boolean;
}

/**
 * Error boundary for individual dashboard widgets.
 * Catches rendering errors and shows a compact fallback UI.
 */
export class WidgetErrorBoundary extends Component<
  WidgetErrorBoundaryProps,
  WidgetErrorBoundaryState
> {
  state: WidgetErrorBoundaryState = { hasError: false };

  static getDerivedStateFromError(): WidgetErrorBoundaryState {
    return { hasError: true };
  }

  handleReset = () => {
    this.setState({ hasError: false });
  };

  render() {
    if (this.state.hasError) {
      return (
        <Card className="h-full min-h-70">
          {this.props.title && (
            <CardHeader className="pb-2">
              <CardTitle className="text-sm font-medium">{this.props.title}</CardTitle>
            </CardHeader>
          )}
          <CardContent className="flex flex-1 flex-col items-center justify-center py-8">
            <AlertTriangle className="h-6 w-6 text-muted-foreground mb-2" />
            <p className="text-sm text-muted-foreground mb-3">Widget failed to load</p>
            <Button variant="outline" size="sm" onClick={this.handleReset}>
              <RefreshCw className="mr-2 h-3 w-3" />
              Retry
            </Button>
          </CardContent>
        </Card>
      );
    }

    return this.props.children;
  }
}
