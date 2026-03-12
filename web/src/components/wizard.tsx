import { Check } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";

export interface WizardStep {
  id: string;
  label: string;
}

interface WizardProps {
  steps: WizardStep[];
  currentStep: number;
  /** Called when the primary forward action is triggered. */
  onNext: () => void;
  /** Called when Back is triggered. Omit on first step. */
  onBack?: () => void;
  /** Called when Cancel is triggered. */
  onCancel: () => void;
  /** Label for the primary forward button. Defaults to "Next". */
  nextLabel?: string;
  /** Disables the primary forward button. */
  canNext?: boolean;
  /** Shows a spinner on the primary forward button. */
  nextLoading?: boolean;
  /** Extra buttons rendered between Back and the primary action (e.g. "Test"). */
  extra?: React.ReactNode;
  /** Slot rendered above the footer buttons (e.g. an inline result banner). */
  aboveFooter?: React.ReactNode;
  /** The current step's content. Rendered in the scrollable body. */
  children: React.ReactNode;
}

export function Wizard({
  steps,
  currentStep,
  onNext,
  onBack,
  onCancel,
  nextLabel = "Next",
  canNext = true,
  nextLoading = false,
  extra,
  aboveFooter,
  children,
}: WizardProps) {
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {/* Step indicator */}
      <WizardStepIndicator steps={steps} currentStep={currentStep} />

      {/* Scrollable step content */}
      <div className="mt-5 flex min-h-0 flex-1 flex-col overflow-y-auto p-1">{children}</div>

      {/* Footer */}
      <div className="mt-4 shrink-0 space-y-3 border-t pt-4">
        {aboveFooter}
        <div className="flex items-center gap-2">
          <Button type="button" variant="outline" onClick={onCancel}>
            Cancel
          </Button>
          {extra}
          <div className="ml-auto flex gap-2">
            {onBack && (
              <Button type="button" variant="outline" onClick={onBack}>
                Back
              </Button>
            )}
            <Button type="button" onClick={onNext} disabled={!canNext || nextLoading}>
              {nextLoading && <Spinner className="mr-2 h-4 w-4" />}
              {nextLabel}
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}

function WizardStepIndicator({ steps, currentStep }: { steps: WizardStep[]; currentStep: number }) {
  return (
    <ol className="flex items-center">
      {steps.map((step, index) => {
        const isActive = index === currentStep;
        const isComplete = index < currentStep;
        const isLast = index === steps.length - 1;

        return (
          <li key={step.id} className={cn("flex items-center", !isLast && "flex-1")}>
            <div className="flex shrink-0 items-center gap-2">
              <div
                className={cn(
                  "flex h-6 w-6 items-center justify-center rounded-full text-xs font-semibold transition-colors",
                  isActive
                    ? "bg-primary text-primary-foreground"
                    : isComplete
                      ? "bg-primary/15 text-primary"
                      : "border-2 border-border text-muted-foreground",
                )}
              >
                {isComplete ? <Check className="h-3 w-3" /> : index + 1}
              </div>
              <span
                className={cn(
                  "whitespace-nowrap text-sm transition-colors",
                  isActive ? "font-medium text-foreground" : "text-muted-foreground",
                )}
              >
                {step.label}
              </span>
            </div>

            {!isLast && (
              <div
                className={cn(
                  "mx-3 h-px flex-1 transition-colors",
                  isComplete ? "bg-primary/30" : "bg-border",
                )}
              />
            )}
          </li>
        );
      })}
    </ol>
  );
}
